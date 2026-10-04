# Operations

Building, running, testing, and releasing mooloop, and everything specific to
doing it from an agent on Adam's machine.

`AGENTS.md` is the workflow contract these commands serve — worktrees, the
verification ladder, and when to climb it. This file is the mechanics.

## Start A Piece Of Work

`AGENTS.md` has the rules: when a worktree is needed, the branch prefixes,
and what to do with a dirty tree. From a clean, current `main`:

```sh
git status --short --branch
git pull --ff-only origin main
git worktree add ../mooloop-worktrees/<short-name> -b <type>/<short-name> main
cd ../mooloop-worktrees/<short-name>
git config core.hooksPath .githooks
```

For several worktrees, sharing the build cache avoids rebuilding dependencies
in each one:

```sh
export CARGO_TARGET_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/mooloop/cargo-target"
```

Put that in your shell setup if you want it permanent. The target directory is
machine-local output; do not commit it.

## Cargo: The Usual Commands

```sh
# Fast compile/type check; good before a commit.
cargo check --workspace --all-targets -j 2

# Build the application in the development profile.
cargo build -p mooloop-app -j 2

# Run the application. On Linux this needs JACK or PipeWire's JACK layer;
# on macOS it plays through Core Audio.
cargo run -p mooloop-app --bin mooloop -j 2

# Optimized build, suitable for a local performance or packaging check.
cargo build --release -p mooloop-app -j 2

# This tree is not rustfmt-normalized, and CI does not run rustfmt. Do not use
# a workspace-wide `cargo fmt` or `cargo fmt --check` as a verification gate:
# it rewrites or reports unrelated legacy code. Keep touched Rust formatted
# locally and use the compile, test and clippy rungs below.

# Lint the whole workspace exactly as CI does.
cargo clippy --workspace --all-targets -j 2 -- -D warnings
```

When the answer you need from a command is whether it passed, put
`scripts/exit-code` in front of it (`AGENTS.md` explains why). It logs the
output, prints the failure lines, and exits with the command's own code, and
it composes with the wrappers below:

```sh
scripts/exit-code cargo test -p mooloop-dsp
scripts/exit-code --tail 100 cargo clippy --workspace --all-targets -- -D warnings
scripts/exit-code scripts/cargo-capped test -p mooloop-ui
```

For a narrow change, test the crate you touched. `mooloop-ui` is the heavy
one, so retain its explicit job cap:

```sh
cargo test -p mooloop-dsp -j 2
cargo test -p mooloop-engine -j 2
cargo test -p mooloop-ui -j 2
```

## Cargo limits

Never run Cargo build, test, or Clippy commands concurrently on this machine.
Memory is the constraint; `nice` does not solve that. Keep the workspace
development profile's capped debug information intact.

Run Cargo through `scripts/cargo-capped`, which puts the run in a
memory-bounded cgroup. It costs nothing in speed, and it is the only thing
that keeps a heavy run from freezing the desktop instead of just failing.
Prefix `MOOLOOP_CAP_STATS=1` to print peak memory.

```sh
scripts/cargo-capped check -p mooloop-ui
scripts/cargo-capped clippy -p mooloop-ui --all-targets
scripts/cargo-capped test -p mooloop-ui -j 2
```

Two distinct memory problems live here, and the fix for one does not help the
other:

- **Linking**, which is what makes `cargo test` expensive: seven `mooloop-ui`
  test binaries link at once. That is what `.cargo/config.toml`'s job cap,
  `mold`, and the dev debug-info cap address. Cap jobs and prefer one relevant
  test target, as above.
- **Checking**, which never links, so none of the above applies to it. A
  `mooloop-ui` check peaks at 3.4 GB after an edit and 5.2 GB cold, in a
  *single* rustc process handling the one huge module `build.rs` generates
  from `ui/main.slint` (about 395,000 lines of Rust, 96% of what `mooloop-ui`
  compiles). Job count cannot subdivide one process, so lowering `jobs` does
  not help; only the cgroup bound does.

`.cargo/config.toml` limits default Cargo jobs to three. Do not raise the
limit.

Do not set `CARGO_INCREMENTAL=0` to save memory. It is the obvious guess and
it is measurably wrong here: on the same `.slint` edit it cost 4.58 GB and
2m01s, against 3.42 GB and 41s with incremental left on. A cloud container is
the one place that inverts (see *Working In A Cloud Container*); on this
machine, leave it on.

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
keyed by absolute path, so two worktrees may build remotely at the same time.
Cargo's job cap is lifted to the remote core count.

### Do not edit while a remote build is running

**A remote build that finishes *after* you edit a file will make the next run
ignore that edit.** `antibox` rsyncs with timestamps preserved, and Cargo
decides freshness by comparing a source's mtime against its build artifacts,
so the remote ends up with artifacts newer than your edited sources. For a
`.slint` edit the build *script* is the thing skipped, so `ui/main.slint` is
silently the old one while your Rust is the new one. It presents as a compile
error that makes no sense (a struct just added to `main.slint` reported as
`cannot find type`), with the Slint build script's warnings in the log
**replayed from cache, which is what makes the log look like it ran**.

So: **do not start a background remote build and then keep editing.** If you
have, `touch` what you changed before the next run, which is enough to move the
mtimes past the artifacts:

```sh
touch crates/mooloop-ui/ui/*.slint crates/mooloop-ui/src/*.rs
```

`scripts/antibox --clean` also fixes it and costs a cold build. Prefer the
touch. The habit that avoids it entirely is to launch a remote run when you
have *stopped* editing -- which is also when its result means something.

### Which cache a run gets

sccache and incremental compilation cannot both be on, so `antibox` picks one
per run from the profile. Measured on the box after a one-line edit in
`mooloop-session`:

| Command | sccache | incremental |
| --- | --- | --- |
| `cargo test -p mooloop-session` | 31 s | **18 s** |
| `cargo test --workspace --exclude mooloop-ui` | 141 s | **43 s** |
| `cargo test --workspace` | 332 s | **118 s** |
| `cargo build --release -p mooloop-app` | **522 s** | 672 s |

Dev-profile `check`, `test` and `clippy` therefore get incremental
compilation, and release builds get sccache. `--incremental` and
`--no-incremental`, or `$MOOLOOP_INCREMENTAL=1|0`, override that.

Dependencies are shared across checkouts by sccache, which caches each `rustc`
invocation under a hash of its inputs, so a checkout whose sources differ gets
a recompile, never another checkout's artifact. A shared target directory is
not used for that reason: two checkouts of this workspace share package names
and versions, and the second links against the first's stale `mooloop-core`.
`--no-sccache` or `$MOOLOOP_NO_SCCACHE=1` bypasses the wrapper;
`$MOOLOOP_SCCACHE_SIZE` changes the 40G cap.

Pull artifacts back with `--pull`, which is how remote UI snapshots work:

```sh
scripts/antibox --pull /tmp/window.ppm \
  env SLINT_BACKEND=winit-software MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm \
  cargo test -p mooloop-ui --test playlist_snapshot
```

`--clean` discards the remote checkout and its target cache but keeps the
sccache dependency cache; `$MOOLOOP_REMOTE_TARGET` moves the target directory
elsewhere. `--host` and `$MOOLOOP_REMOTE_HOST` point at a different ssh host.
Anything needing JACK, a real audio device, or the live compositor still
belongs on this machine.

For a runnable build of the current tree:

```sh
scripts/antibox --dev-bin                               # ./bin/mooloop-dev
scripts/antibox --release-bin                           # ./bin/mooloop-test
scripts/antibox --release-bin /tmp/mooloop-candidate    # somewhere else
```

Both build the `mooloop` binary on the box, strip it, and copy it here. The
dev profile is tuned to be playable (`opt-level = 1` workspace wide) and
builds in about 1 m 52 s on the box against 522 s for release. **A dev binary
is the default way to hear a change**, and the one to hand Adam to listen to;
keep `--release-bin` for judging performance. A stripped binary has no
backtrace symbols, so add `--keep-symbols` when diagnosing a crash.

`scripts/mooloop-run` does the whole cycle -- build on the box, copy the binary
down, run it here against JACK -- and falls back to a capped local build if the
box is unreachable. Its default, the dev profile, is the one to use:

```sh
scripts/mooloop-run              # dev profile
scripts/mooloop-run --release
scripts/mooloop-run --local      # never touch the box
```

### Keeping the box from filling up

Each run records the checkout it came from, and `scripts/antibox --prune`
deletes the remote caches whose checkout no longer exists. A run that finds
the box above 90% full warns, in one line beginning `antibox: warning --`
printed before the build starts. **It is easy to grep past**: the failure it
predicts arrives minutes later as
`error: failed to write ... No space left on device`, which reads like a
compiler error and is not one. Read the first lines of a remote run's log, not
only the compiler-shaped ones.

**One task per worktree means one remote cache per worktree**, and a
`mooloop-ui` target is 20-25 GB. `--prune` only reclaims a cache once its
*local* worktree is gone, so run it just after `git worktree remove`, not when
a build fails.

## All Tests And Release Verification

This is the full integration suite. Run each line after the previous one has
finished.

```sh
cargo test --workspace -j 2
cargo clippy --workspace --all-targets -j 2 -- -D warnings
cargo run -p mooloop-app --bin engine-selftest -j 2
MOOLOOP_AUTODRIVE=1 cargo run -p mooloop-app --bin mooloop -j 2
```

The last command exercises the app's automated smoke path.
`MOOLOOP_AUTODRIVE_RECORD=1` is its twin for recording: a kick plays, a new
sampler takes the master as its AUDIO input and records one clip bar from its
RECORD page, and the report says whether the page showed the pre-roll and the
take and whether the take became the sampler's sample as one "Record Take"
undo step. Give it a throwaway `MOOLOOP_CONFIG_DIR` -- the take is written
into that folder's `recordings/`. It drives the callbacks in-process because
the AUDIO row is a popup the MCP tools cannot click.

For UI work, add a software-rendered snapshot of the real window; see
*Capturing the real widgets* below.

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

`cargo build -p mooloop-ui` costs about four minutes for any edit, because
rustc recompiles the whole generated module either way, so do the adjusting
here and build once at the end.

Its limits are worth knowing before you trust a render. Anything driven by a
Rust model -- the piano grid, mixer strips, the device rack's contents -- draws
empty, because only the `.slint` side exists; a device face renders its chrome
and controls but not its curve. It is for spacing, colour, proportion and
typography, not for interaction or live data.

The viewer installs its own software backend, so no display, compositor or
`agent` workspace is involved. Sketches and their PNGs land in `$TMPDIR`,
outside the repo -- keep them there, they are working notes rather than
artefacts.

### Capturing the real widgets

Sketching stops where a Rust model starts. For anything model-driven, the
`mooloop-ui` test suite already builds the real window and can be asked to
write its snapshot to disk. Every one of these follows the same shape — the
test always runs and asserts; setting an environment variable additionally
writes the PPM it rendered:

```sh
MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm \
  cargo test -p mooloop-ui --test playlist_snapshot
magick /tmp/window.ppm /tmp/window.png
```

There are around fifty of these across twenty test files — every source face
and its pages, the mixer, the modulation shelf and each module kind, the
preferences pages, the effect rack scrolled and unscrolled, before/after shots
either side of a drag. Find the one you want rather than adding another:

```sh
rg -o 'MOOLOOP_[A-Z_]+_SNAPSHOT' crates/mooloop-ui/tests | sort -u
```

The variable name says which test file to run; `rg -l <VARIABLE>
crates/mooloop-ui/tests` gets you there. Add a new one only when no existing
state shows what you changed, and follow the surrounding convention: assert
something, and write the image as a side effect.

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
`take_screenshot`, and event recording. They are `i-slint-backend-testing`'s
`ElementHandle` API over HTTP. This is the only view of the interface with the
real Rust models and the engine behind it: a click lands on the same code path
Adam's click lands on.

```sh
scripts/mooloop-mcp              # build on the box, start headless, print the endpoint
scripts/mooloop-mcp --status
scripts/mooloop-mcp --stop
scripts/mooloop-mcp --window     # a real window on the `agent` output instead
```

It is headless by default -- no compositor, works while the screen is locked,
and a windowed run on a machine with no display cannot screenshot at all. JACK
is not optional either way: the engine starts before the UI and takes the
process down with it if it fails, so a port that never answers usually means
the log the script names, not the server. The one xrun usually reported while
connecting to JACK on startup is the connection, not a fault in what you are
testing.

`.mcp.json` registers the endpoint for Claude Code, so the tools appear in a
session started while the application is up; the entry is dead the rest of the
time. `curl` is the reliable way to drive it from a script:

```sh
curl -s -X POST http://127.0.0.1:9010/mcp -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"list_windows","arguments":{}}}'
```

The two handle kinds have the same `{index, generation}` shape and are not
interchangeable. `list_windows` returns a window handle;
`get_window_properties` on it returns `rootElementHandle`; `get_element_tree`
takes that *element* handle and walks down from it, a thousand elements at a
time. Elements come back with `.slint` ids and accessible labels --
`MainWindow::menu-bar`, `ToolButton::tap`, "Show the step grid or the mixer:
Mixer" -- and absolute positions that line up with the screenshot, so search
the tree rather than guessing coordinates.

`click_element` is a real pointer event and leaves the pointer where it
clicked, so the next screenshot may show a hover tooltip.

**It does not reach inside a `PopupWindow`.** A `PickerChip` or `BusPicker`
row appears in the element tree with correct positions, but clicking it does
nothing: the popup closes and the value is unchanged, which looks exactly like
a callback that never fired. The widget is not broken (`BusPicker` reports its
choice *before* closing and fails here identically). Verify a picker some
other way — a snapshot test that sets the model directly, or a unit test of
the handler's session half — and use the live application for everything
outside a popup.

Two switches gate the server, and both are off in anything released. The
`mcp` feature on `mooloop-app` compiles it in and makes `crates/mooloop-ui`'s
`build.rs` emit element debug info, without which every tool that names an
element fails at runtime. `$SLINT_MCP_PORT` starts it; unset, the code is
inert. It binds `127.0.0.1`, validates that the request origin is local, and
has no authentication -- it is a development tool, and the packaging never
turns the feature on.

Flipping the feature costs a full rebuild of the generated Slint module in
either direction -- 13m05s on the box for a cold release build -- which is why
the script builds there by default and keeps its binary at `bin/mooloop-mcp`
rather than in `target/`.

## Working In A Cloud Container

An agent session on Claude Code's web runner gets a fresh Ubuntu container
that is neither the laptop nor the box: a Rust toolchain, an empty `target/`,
none of the audio, graphics or font development headers, and no `mold`, which
`.cargo/config.toml` pins as this target's linker. Everything else in this
document still holds, `scripts/exit-code` included; the memory rules, the
incremental rule and the timings do not.

### Claude Code on the web

`.claude/hooks/session-start.sh`, registered as a `SessionStart` hook in
`.claude/settings.json`, installs the packages. It runs synchronously, because
the first thing a session does is usually a Cargo command and an async install
loses that race. It also sets `core.hooksPath` (a new container is a new clone
every time) and writes `CARGO_BUILD_JOBS=1` into the session environment.

Its package list is a copy of the one `.github/workflows/ci.yml` installs, plus
`mold`. **CI is the reference; the hook is the copy.** A build that fails there
for a missing header needs both edited.

Without those packages, two things stop the build before anything compiles,
and neither error names its cause:

- **No `mold`:** every link fails with `collect2: fatal error: cannot find
  'ld'`, though `ld`, `ld.lld` and `ld.gold` are all present. Override the
  config's target rustflags from the environment rather than editing the file
  (CI does the same with `RUSTFLAGS: ""`):

  ```sh
  export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=lld"
  ```

- **No JACK headers:** `jack-sys`'s build script panics when pkg-config cannot
  find JACK. Install CI's list, after `apt-get update` or stale package lists
  404 on part of it. `libjack-jackd2-dev`, `libfontconfig-dev` and
  `libxkbcommon-dev` alone are enough to build and test `mooloop-ui`.

### The container's limits are not the laptop's

| | Laptop | Web container |
| --- | --- | --- |
| RAM | 16 GB | 15 GB |
| Swap | 8 GB zram | **none** |
| Cores | 8 threads | 4 |
| Disk | the machine's | a finite session allowance |
| `scripts/cargo-capped` | bounds the run | no systemd; runs uncapped |

The missing swap is the whole difference: an overshoot is an OOM kill rather
than a slowdown, and it can land on the session itself. Cargo reports a killed
`rustc` as `error: could not compile mooloop-ui`; a `signal: 9, SIGKILL` with
no diagnostic above it is the OOM killer, not the compiler.

One cold `mooloop-ui` `rustc` peaks at 5.8 GB for a check and **8.5 GB** for a
test build, so two do not fit, and `CARGO_BUILD_JOBS=1` overrides
`.cargo/config.toml`'s `jobs = 3`. How many of those units a command has
decides whether overriding that is safe:

| Command | Big rustc units | Safe at |
| --- | --- | --- |
| `cargo check -p mooloop-ui` | 1 (nothing else depends on it) | any job count |
| `cargo test`/`build -p mooloop-ui` | 7, one per test binary | one job |
| anything not touching `mooloop-ui` | 0 | any job count |

**So pass `-j 4` explicitly for crates other than `mooloop-ui`**, and leave
the default alone for anything that compiles `mooloop-ui`:

```sh
cargo test -p mooloop-dsp -j 4    # 666 tests, 25s including the cold dep build
cargo test -p mooloop-ui          # leave this one at the default
```

Splitting a pass -- workspace `--exclude mooloop-ui` at full parallelism, then
`mooloop-ui` alone at one job -- keeps most of the speed. Do not raise the
default to speed up a slow workspace run: the workspace run is the command
that gets killed.

On four container cores a run is a verification pass, not an iteration loop
-- decide what to run once, background it, and do other work while it runs:

| Run | Wall |
| --- | --- |
| `cargo check -p mooloop-ui`, cold | 9m54s |
| `cargo check -p mooloop-ui`, dependencies built | 4m50s |
| `cargo test -p mooloop-ui --lib`, including the test profile's dependencies | 11m25s |
| `cargo clippy -p mooloop-ui --all-targets -- -D warnings` | 14m56s |
| `cargo test -p mooloop-ui --no-run`, cold | 33m42s |

### When the disk fills

`df` is misleading when the session's allowance runs out: "Avail" reads 0
against a low "Used", and deletes still succeed while writes fail. It may not
show as a cargo error at all; the first sign can be the harness losing a
command's output, because its own temp files have nowhere to go. A built
`target/` is about 22 GB after one `cargo test -p mooloop-ui --no-run` (8.4 GB
of it incremental state) and 30 GB after a `cargo test --workspace` (15 GB).

```sh
rm -rf target/debug/incremental   # ~9 GB back, and a rebuild regenerates it
cargo clean -p mooloop-ui         # expensive; those seven binaries are the rest
```

**The first does not invalidate anything already built.** It costs the next
*changed* rebuild its incremental cache, nothing more, and it works from
inside an already-wedged session. The hook does it itself when it fires (on a
resume or compact) with under 8 GB free; `MOOLOOP_WEB_RECLAIM_BELOW_GB` moves
the threshold.

The hook leaves `CARGO_INCREMENTAL` on, because turning it off trades disk for
memory. **For a verification pass, export `CARGO_INCREMENTAL=0`** -- in a
container, and only there. It inverts the laptop rule under *Cargo limits*
because a verification pass compiles each crate once from cold, so the
incremental state is never reused, and here disk runs out first. It is
survivable only together with one job for `mooloop-ui` -- one 4.6 GB `rustc`
fits, four do not. Do not carry either setting to the other machine.

Even so, a full rung 4 only just fits: `cargo test -p mooloop-ui` and its
clippy, on top of the workspace runs, add 11 GB and half an hour and left
1.8 GB free. Budget for it, or split rung 4 across two sessions.

**Several team agents building branches through one shared
`CARGO_TARGET_DIR` fill the disk faster still:**

- rustc keeps the previous incremental session beside the new one until that
  unit's next compile, so a unit compiled again is roughly disk-neutral and a
  *new* one -- an integration test binary, a `clippy --all-targets` check
  unit -- is not.
- `--exclude mooloop-app` changes feature unification and rebuilds external
  crates (`zbus` among them) under new hashes, costing more disk than it
  saves. Leave rung 3 as `--exclude mooloop-ui`, or skip it and let CI run
  the workspace.
- CI is the other verifier (Linux and macOS: check, clippy `-D warnings`, the
  full workspace tests). When the disk cannot hold a `mooloop-ui` test build,
  read every changed Rust line against the APIs it calls before pushing, and
  run `scripts/slint-sketch crates/mooloop-ui/ui/main.slint` on the tree being
  pushed.

## Developing On macOS

The workspace builds and runs on a Mac, where the engine plays through Core
Audio instead of JACK. The Xcode command-line tools and `rustup` are all it
needs, and `mold` is not used.

MIDI input comes from Core MIDI with nothing to set up; each source is logged
as `listening to the MIDI input "<name>"`. If a keyboard plays nothing, look
for that line first; Audio MIDI Setup's MIDI Studio window shows whether macOS
sees the device at all.

**Audio input needs the microphone permission, and macOS refuses it
silently.** The log says `listening to the audio input "<name>"` or
`no audio input: <reason>`, and a refused permission names Privacy & Security;
the channel sidebar's AUDIO row then lists no input source. The driver retries
once a second, so granting the permission should be picked up without a
relaunch -- **untested**, and macOS may cache a TCC decision for the life of a
process. If the row does not appear within a second or two of granting it,
restart mooloop and say so here.

**One shared build cache, at `~/.cache/cargo-target`, on the external SSD**,
set as `CARGO_TARGET_DIR` in `~/.zshenv`. The path is a **symlink** into
`/Volumes/Extended SSD`, and that is load-bearing: the volume name contains a
space, and an autoconf build script (`mp3lame-sys`, in this workspace) handed
a path with a space fails in ways that read as a broken toolchain. Cargo does
not canonicalise `CARGO_TARGET_DIR`, so `OUT_DIR` arrives space-free. The
guard in `~/.zshenv` tests the *volume*, so a detached drive leaves
`CARGO_TARGET_DIR` unset and cargo falls back to a per-project `target/`
instead of quietly filling the boot disk.

**The SSD is on a USB 2.0 cable** (~41 MB/s against the internal disk's
~904 MB/s): a `mooloop-app` rebuild after touching `mooloop-engine/src/lib.rs`
took 3 m 29 s of wall clock for 23.6 s of CPU. Judge any build time on this
machine against that before concluding something in the workspace got slower.

**`cargo` reaches non-interactive shells through `~/.zshenv`, not `.zshrc`.**
Homebrew's `rustup` is keg-only, so `$(brew --prefix rustup)/bin` has to be on
`PATH`; without it `rustup` resolves while `cargo` does not, which reads as a
broken toolchain. `~/.zshenv` is the only startup file zsh reads for every
invocation (`.zshrc` returns early when not interactive, and `.zprofile` is
login shells only), and a `zsh -c "cargo ..."` is how scripts, editors and
coding agents run things. It is machine-local; a fresh clone on another Mac
needs it made again.

The JACK adapter does not compile on a Mac, so an edit to it, or to anything
else behind `cfg(not(target_os = "macos"))`, goes unchecked there.
`scripts/linux-check` checks the Linux build from the Mac instead:

```sh
scripts/linux-check                     # cargo check -p mooloop-engine --all-targets
scripts/linux-check clippy -p mooloop-engine --all-targets -- -D warnings
```

It needs `rustup target add x86_64-unknown-linux-gnu` and `brew install zig`.

The other way round, **the Mac build can be checked from the build box**, as
long as nothing has to link: `cargo check` and `cargo clippy`, not `test`. The
box's pinned toolchain has the `aarch64-apple-darwin` std. The one obstacle is
`mp3lame-sys`, whose build script runs autoconf with a C compiler for the
target: a wrapper on the box that drops `-arch` and `-mmacosx-version-min=`
and calls the host `cc` gets past it, and the objects it builds are never
linked. The wrapper is not kept, so recreate it at `/tmp/moo237/cc`:

```sh
scripts/antibox --no-incremental env CC_aarch64_apple_darwin=/tmp/moo237/cc \
  cargo clippy --locked --target aarch64-apple-darwin -p mooloop-engine --all-targets -- -D warnings
```

Count the `Checking mooloop-engine` line in the log before trusting a green
run. CI's macOS job is still the check that counts.

`scripts/cargo-capped` finds no memory cgroup on macOS and runs Cargo uncapped,
so builds stay one at a time there too. `scripts/antibox` works from a Mac that
can reach the box, but what it builds are Linux binaries: build locally to run
the application.

## Measuring what a block costs

`crates/mooloop-engine/src/block_cost.rs` prints nanoseconds per
`process_block` against the block's own real-time budget, across four buffer
sizes and four channel counts, with the channels playing and idle. It is two
`#[ignore]`d tests rather than a benchmark harness, so it needs asking for:

```sh
scripts/antibox cargo test -p mooloop-engine --release block_cost \
  -- --ignored --nocapture --test-threads=1
```

`--release` because a debug build measures the wrong program, and
`--test-threads=1` because the two tests otherwise contend and inflate each
other by a third. Run it before and after anything on the block path. The
figures in the `Sep 5 (last)` entry of `docs/JOURNAL.md` are what it said on
the build box, and are the comparison to beat rather than to reproduce -- the
laptop's numbers are its own.

## Measuring what the UI thread costs

`MOOLOOP_PROFILE_UI` (`crates/mooloop-ui/src/pump_profile.rs`) times the 8 ms
pump section by section, every frame Slint renders (`BeforeRendering` to
`AfterRendering`, where bindings, layout and drawing run), and every thread's
CPU from `/proc/self/task/*/schedstat`, so the UI thread's share reads beside
the audio thread's. It prints to stderr and costs nothing when unset.

```sh
MOOLOOP_PROFILE_UI=1 mooloop song.mooloop          # a report every 5 s
MOOLOOP_PROFILE_UI=scenario mooloop song.mooloop   # scripted run, then quit
```

`scenario` is for measuring a song with nobody at the window: stopped on the
saved layout, then playing on it, on the mixer, the rack, the playlist and the
steps, then stopped on the mixer and the rack, each for
`MOOLOOP_PROFILE_UI_PHASE_SECS` (default 15), and it quits. Only the song is
read, but the layout and settings are saved on quit, so point
`MOOLOOP_CONFIG_DIR`/`MOOLOOP_STATE_DIR` at a copy. Judge it on a release
binary (`scripts/antibox --release-bin --keep-symbols`, which keeps `perf`
symbols) in a real GPU window. Headless sway keeps it off the desktop
(`WLR_BACKENDS=headless sway -c <config>` with an `exec` line), and
`PIPEWIRE_REMOTE=none` gives the null driver, so nothing sounds.

What to read: `frames/s` equal to `ticks/s` while nothing visible changes
means something is marking the window dirty every tick, and FemtoVG repaints
the whole window each time. The 2026-09-25 survey's figures are on MOO-256
and MOO-257.

## Recordings

A take is written, as it records, as a 32-bit float stereo WAV named
`<UTC date>-<time>-<channel>.wav` (`mooloop_session::take::TakeRecorder`); a
second take wanting the same name in the same second gets `-2`, `-3` before
the extension. The interface points the recorder at
`settings::recordings_dir()`: **`$XDG_DATA_HOME/mooloop/recordings`**
(`~/.local/share/...`) on Linux, and beside the settings on macOS and Windows.
`$MOOLOOP_DATA_DIR` overrides it, and a `$MOOLOOP_CONFIG_DIR` with no data
directory keeps it inside that config directory. `docs/CURRENT.md` describes
what happens to takes after that.

## Diagnostic Log

The app writes a levelled record of what it does to stderr: what it opened and
saved, every correction the repair pass applied, xruns, and any failure.

```sh
MOOLOOP_LOG=debug cargo run -p mooloop-app --bin mooloop -j 2
```

`MOOLOOP_LOG` takes `error`, `warn`, `info` (the default), or `debug`;
`MOOLOOP_DEBUG=1` also means `debug`.

**Every run also writes everything, `debug` included, to a log file**:
`$XDG_STATE_HOME/mooloop/mooloop.log` (by default
`~/.local/state/mooloop/mooloop.log`; `~/Library/Logs/mooloop/` on a Mac). It
appends across runs and rolls to `mooloop.log.1` past 4 MB. Preferences →
Developer shows the path. `MOOLOOP_STATE_DIR` moves it; so does
`MOOLOOP_CONFIG_DIR`, which keeps a disposable run's log beside its settings.

**A panic leaves a crash report** in `crashes/` beside the log:
`crash-<UTC stamp>.txt`, with the build, the thread, the message and a
backtrace, for the first panic of a run (the log gets a line for each of the
first sixteen). The newest ten reports are kept.

**SIGTERM, SIGINT and SIGHUP quit the way Quit does**, without its dialog: the
log says `quitting on SIGTERM` and whether the song had unsaved changes, which
are not saved, and a take still recording is finished. A second signal ends it
at once.

**Autosave** folders, `autosave/<UTC stamp>-<pid>-<n>/`, sit beside the log
(on a Mac, beside the settings instead of in Logs), one per running mooloop,
which holds an OS lock on its `lock` file while it runs and, while the song is
unsaved, writes `song.mooloop` and `about.txt` there once a minute. A folder
whose lock can be taken belongs to a mooloop that has ended. Deleting the
`autosave/` folder while no mooloop runs is always safe. A song that cannot be
saved also cannot be autosaved, and the log says why. `docs/CURRENT.md` has
the rest of its behaviour.

A song that cannot be saved is written to `~/.config/mooloop/quarantine/`
anyway, with a `.txt` beside it holding the same explanation the dialog showed.
Open one with `toml` in hand rather than the app: loading it through the app
repairs it, which is what destroys the evidence.

Nothing here may be called from the audio thread; see
`crates/mooloop-core/src/log.rs`.

### The Output Went Missing

A saved output outlives the thing it names -- unplugged headphones, a changed
ALSA profile, an audio server that renamed its nodes -- and connecting to
nothing is the one failure with no symptom: the meters move, the transport
rolls, and there is silence. So under JACK the output follows Adam's rule
(2026-09-22):

- **Stay on the most recently picked output that is there.** Preferences
  remembers the last eight outputs picked (`earlier-outputs` in the driver's
  section of `settings.toml`, beside `output-port-l`/`-r`). At launch, and
  whenever the outputs are connected to nothing, mooloop connects to the most
  recent of them whose ports are all in the graph.
- **Never move an output that is connected** -- to anything, including a link
  made by hand in a patchbay -- even for a device picked more recently.
- **Something over nothing.** With no remembered output there, the graph's
  first working stereo destination that is not mooloop itself is taken,
  unranked. Preferring speakers over HDMI would be a guess about a machine
  the engine cannot see.

```text
warn  audio  the saved audio output "..." is not available; connected to
             "alsa_output...HiFi__Speaker__sink:playback_FL" instead.
             Preferences -> Audio picks a different one
info  audio  the audio output "..." is not connected; playing through "..."
```

The check runs on the control thread, half a second after the port graph last
changed and once a second otherwise. Turning auto-reconnect off in Preferences
stops everything but the choice at launch. A new output is connected before
the old one is let go, so a choice in Preferences that will not connect leaves
the current one playing. Core Audio is not on this rule: it returns to the
picked device when it comes back, even while the system default is playing.

`pw-link -l | grep mooloop` lists the links; no output at all means the
outputs are connected to nothing.

### The Audio Stopped, Or Never Started

With no JACK -- no libjack installed, or no server answering -- mooloop opens
anyway, on a null driver that renders into nothing, and asks "mooloop is
running with no audio" with Reconnect. Once running, it asks "The audio
stopped" when the JACK server shuts the client down (what a PipeWire restart
does), changes its sample rate, or the callback has not run for three seconds.
The log says which:

```text
warn  audio  no JACK server is running; start PipeWire or JACK; running with no audio device
warn  audio  The audio stopped: the audio server shut down
info  audio  reconnected at 48000 Hz: Running
```

Reconnect builds a new engine at the server's current rate and installs the
open song, transport stopped. Dismissed, the question stays in the status bar,
and Preferences -> Audio -> Refresh reconnects. A device that panics in the
callback is not this: that block plays as silence, the log counts it, and the
next block renders.

### Reading An Audio Dropout

A dropout has two possible causes and they need opposite fixes, so the log
reports both numbers rather than the xrun alone. Once a second, when there is
something to say:

```text
warn  audio  audio dropout in the last second: 3 of 47 blocks over budget,
             0 late wake-ups, 1 xruns reported (load 34% mean, 118% worst
             block, worst wake-up 1.1x the block period)
```

**Blocks over budget** means the engine did more work than the buffer size
allows. A block of 1024 frames at 48 kHz has 21.3 ms; if the worst block wants
more, the fix is a larger buffer or a lighter project. **Late wake-ups** mean
the opposite: the block was cheap and the operating system did not run the
audio thread in time. Nothing about the project will help that. The xrun
count is a count, not a flag: several arrive between two blocks when a machine
stalls.

The first thing to check when late wake-ups appear is whether the callback is
realtime at all. It is warned about once per run:

```text
warn  audio  the audio callback is running on an ordinary time-shared thread,
             not a realtime one; ...
```

Under PipeWire this is usually `rtkit` having demoted every realtime thread on
the machine after its canary starved — a heavy local build is enough to cause
it, and nothing undoes it automatically. `journalctl -b -u rtkit-daemon` shows
it as "Demoting known real-time threads", and
`scripts/procs --threads data-loop` prints each matching thread's current
policy and realtime priority (the same two stat fields `chrt -p` reads). The
status bar's "not realtime" asks the kernel the same thing about once a
second; if it and `chrt -p` disagree for longer than a second, that's a bug.

`systemctl --user restart pipewire pipewire-pulse wireplumber` asks again.
Putting the user in the `pipewire` group is the durable fix: Fedora's
`/etc/security/limits.d/25-pw-rlimits.conf` grants that group `rtprio 70`
directly, so PipeWire stops depending on rtkit's judgement.

## Commit, Merge, And Tidy Up

Commit small, buildable changes from the task worktree (`AGENTS.md` covers
the `CONTRIBUTORS.md` row).

```sh
git status --short --branch
git add <files>
git commit -m "type(area): concise imperative summary"
git push -u origin HEAD
```

When the branch is ready, fast-forward `main`; no merge commits or history
rewrites:

```sh
git -C /home/adam/projects/mooloop pull --ff-only origin main
git -C /home/adam/projects/mooloop merge --ff-only <type>/<short-name>
git -C /home/adam/projects/mooloop push origin main
```

Then remove the now-clean linked checkout and its merged local branch:

```sh
git worktree remove ../mooloop-worktrees/<short-name>
git branch -d <type>/<short-name>
```

`git worktree remove` refuses a dirty worktree. Treat that as a useful stop
sign. Inspect it with `git -C <path> status --short`; only use `--force` when
you have deliberately decided to discard those changes.

## Releases And Tags

The version lives in the root `Cargo.toml` under `[workspace.package]`.
Change it on a normal release branch, commit it, run the full suite above, and
fast-forward it into a clean, current `main`.

In the same commit, **date the version's heading in `CHANGELOG.md`**:
`## 0.1.5 — unreleased (...)` becomes `## 0.1.5 — 2026-09-26`. That section is
the GitHub Release's text, and the workflow's `verify` job fails the tag if
the section is missing or its heading still says "unreleased". Check it before
tagging; this prints exactly what will be published, or says why not:

```sh
scripts/release-notes --changelog vX.Y.Z
```

Then tag the exact `main` commit and push the branch before the tag:

```sh
git -C /home/adam/projects/mooloop pull --ff-only origin main
git -C /home/adam/projects/mooloop status --short --branch

# Confirm Cargo and the tag will agree.
rg '^version = ' /home/adam/projects/mooloop/Cargo.toml

git -C /home/adam/projects/mooloop push origin main
git -C /home/adam/projects/mooloop tag -a vX.Y.Z -m "Mooloop X.Y.Z"
git -C /home/adam/projects/mooloop push origin vX.Y.Z
```

A pushed tag matching `v*.*.*` starts the release workflow. It produces the
`.deb`, `.rpm`, and AppImage packages, plus `Mooloop-<version>-macos-arm64.zip`
(an unsigned Apple Silicon `.app`, ad-hoc signed, built on `macos-latest`), and
attaches them to a GitHub Release whose text is the version's `CHANGELOG.md`
section, with the full commit list since the previous tag attached as
`mooloop-<version>-commits.md`. Because the app is not notarized, macOS
refuses the first double-click. Right-click > Open once, or run
`xattr -dr com.apple.quarantine Mooloop.app`. Every job also runs, without
publishing, from **Run workflow** (`workflow_dispatch`), which is how to check a
workflow change before tagging.

The release workflow does the distribution build against Ubuntu 20.04 for a
glibc 2.31 baseline; the local release build is a useful check, not a
substitute for those artifacts. Adam's ruling (2026-09-25): the Linux release
binary is built for **x86-64-v2** (SSE4.2 and POPCNT: Intel from about 2009,
AMD from about 2011), and only it: tests, CI and every dev or box build stay
at baseline x86-64. `packaging/README.md` has the ruling, the reason, and how
to build a v2 binary locally.

For just an RPM from the current pushed branch, without a release or tag:

```sh
./scripts/build-rpm
```

It requires `gh`, a clean worktree, and `HEAD` already pushed to `origin`.
It downloads the artifact under `.tmp/rpm/`.

## Finding And Cleaning Leftovers

See every linked checkout and branch first:

```sh
git worktree list
git branch --all --verbose
git branch --merged main
```

For a clean, merged local task branch, remove the worktree first, then the
branch, as under *Commit, Merge, And Tidy Up*. If a worktree directory was
already removed outside Git, clear only Git's stale administrative record:

```sh
git worktree prune
git worktree list
```

`git branch -d` is intentionally conservative: it refuses an unmerged branch.
That is a prompt to inspect the branch, not an invitation to reach for `-D`.
Delete a remote branch only after the merged/local state is understood:

```sh
git push origin --delete <type>/<short-name>
```

A leftover *process* -- an application left running from an earlier check, a
stuck `mooloop-mcp`, an orphaned test binary -- is found and stopped with
`scripts/procs`, never with `pgrep -f`/`pkill -f` (`AGENTS.md` says why):

```sh
scripts/procs mooloop            # what is actually running
scripts/procs --kill mooloop     # SIGTERM, wait, report survivors
scripts/procs --kill --force mooloop
scripts/procs --threads data-loop  # thread names, with scheduling policy
```

Finally, clean untracked build output only after looking at it:

```sh
git clean -ndX
git clean -fdX
```

The first command is the dry run. The second removes ignored files, including
unwanted `target/` output, but leaves untracked non-ignored files alone.

## Hook activation

`AGENTS.md` requires `git config core.hooksPath .githooks` once per clone; the
setting is shared by that repository's linked worktrees. Verify it with
`git config --get core.hooksPath`.
