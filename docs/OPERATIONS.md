# Operations

Building, running, testing, and releasing mooloop, and everything specific to
doing it from an agent on Adam's machine.

`AGENTS.md` is the workflow contract these commands serve — worktrees, the
verification ladder, and when to climb it. This file is the mechanics.

> Merged 2026-09-14 from `OPERATIONS.md` and `OPERATIONS.md`. The split
> was "ordinary" versus "agent-specific", and it did not survive contact with
> the fact that **every reader of this file is an agent**. One home, so a
> Cargo question has one answer rather than two that have to be compared.


## Start A Piece Of Work

`main` is the read/merge checkout. Keep it that way. Start by making sure it
is clean and current, then make a sibling worktree for the change:

```sh
git status --short --branch
git pull --ff-only origin main
git worktree add ../mooloop-worktrees/<short-name> -b <type>/<short-name> main
cd ../mooloop-worktrees/<short-name>
git config core.hooksPath .githooks
```

Use `feat/`, `fix/`, `refactor/`, `chore/`, or `spike/` for `<type>`. A dirty
tree is not a mystery to work around: inspect it, then commit it, discard it
deliberately, or split it before starting something unrelated.

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
`scripts/exit-code` in front of it. It logs the output, captures the status
with nothing in between, prints the failure lines, and exits with the
command's own code:

```sh
scripts/exit-code cargo test -p mooloop-dsp
scripts/exit-code --tail 100 cargo clippy --workspace --all-targets -- -D warnings
```

`AGENTS.md` explains why a bare pipe or a trailing `echo` answers for cargo
instead. It composes with the wrappers below -- `scripts/exit-code
scripts/cargo-capped test -p mooloop-ui` is one command with one honest
status.

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
2m01s, against 3.42 GB and 41s with incremental left on. (A web-runner
container is the one place that advice inverts, for disk rather than memory
and at a cost in memory; see the container section near the end. On this
machine, leave it on.)

For scale on where that single module comes from: `slint_build` expands
`ui/main.slint` into roughly 39 MB and 395,000 lines of Rust, so `mooloop-ui`
compiles about 412,000 lines of which 96% are generated. Nothing here can be
tuned below that; `docs/plans/archive/egui-view-layer/00-status.md` measures what the
figures look like without it.

## Claude Code on the web

A web session runs in a fresh Ubuntu container: a bare image with a Rust
toolchain and nothing else. None of the audio, graphics or font development
headers are present, and neither is `mold`, which `.cargo/config.toml` pins as
this target's linker -- so nothing builds at all until they are installed.

`.claude/hooks/session-start.sh` installs them, registered as a `SessionStart`
hook in `.claude/settings.json`. It runs synchronously, because the first
thing a session does is usually a Cargo command and an async install loses
that race; the container image is cached afterwards, so the cost is paid once.
It also sets `core.hooksPath`, which `AGENTS.md` requires once per clone and a
new container is a new clone every time.

Its package list is a copy of the one `.github/workflows/ci.yml` installs, plus
`mold`. **CI is the reference; the hook is the copy.** A build that fails there
for a missing header needs both edited.

### The container's limits are not the laptop's

| | Laptop | Web container |
| --- | --- | --- |
| RAM | 16 GB | 15 GB |
| Swap | 8 GB zram | **none** |
| Cores | 8 threads | 4 |
| Disk | the machine's | a finite session allowance |
| `scripts/cargo-capped` | bounds the run | no systemd; runs uncapped |

The missing swap is the whole difference. On the laptop an overshoot goes to
zram and the run gets slow; here it is an OOM kill, and the kill lands on
whichever process the kernel picks, which can be the session itself.

So the hook writes `CARGO_BUILD_JOBS=1` into the session environment, which
overrides `.cargo/config.toml`. Measured in this container on 2026-09-19, both
runs at one job and both green:

| Run | Peak RSS, one rustc | Wall | CPU |
| --- | --- | --- | --- |
| `cargo check -p mooloop-ui`, cold | 5.8 GB | 9m54s | 99% |
| `cargo test -p mooloop-ui --no-run`, cold | **8.5 GB** | 33m42s | 98% |

Two of that second figure is 17 GB on a 15 GB machine, which is why `jobs = 3`
-- chosen for a laptop with 8 GB of zram behind it -- cannot survive a
`--workspace` run here. The 98-99% CPU says the rest: the expensive unit is one
rustc on the module `slint_build` expands `ui/main.slint` into, and no job
count subdivides one process.

How many of those units a command has is what decides whether overriding the
default is safe:

| Command | Big rustc units | Safe at |
| --- | --- | --- |
| `cargo check -p mooloop-ui` | 1 (nothing else depends on it) | any job count |
| `cargo test`/`build -p mooloop-ui` | 7, one per test binary | one job |
| anything not touching `mooloop-ui` | 0 | any job count |

**So pass `-j 4` explicitly for crates other than `mooloop-ui`**, where the
peak is nowhere near the limit and four cores are otherwise idle, and leave
the default alone for the rest:

```sh
cargo test -p mooloop-dsp -j 4    # 666 tests, 25s including the cold dep build
cargo test -p mooloop-ui          # leave this one at the default
```

Do not raise the default to speed up a slow workspace run. The workspace run is
the command that gets killed. The cost of `jobs = 1` is real -- it serialises
the dependency compilation that would happily run four-wide, and that is most
of the 33 minutes above -- and it is the price of a session that survives.

`CARGO_INCREMENTAL` is left on for the reason given under "Cargo limits":
turning it off trades disk for memory, and memory is the scarcer resource here.

### When the disk fills

A debug `target/` for this workspace is a large fraction of the session's
allowance, and `df` is misleading when it runs out -- "Avail" reads 0 against a
low "Used", because the allowance is spent rather than the disk full. Deletes
still succeed while writes fail.

One `cargo test -p mooloop-ui --no-run` leaves 22 GB behind: 13 GB in `deps/`
(seven test binaries at about 363 MB each, plus a 679 MB `libmooloop_ui`
rlib) and 8.4 GB of incremental state.

```sh
rm -rf target/debug/incremental   # ~9 GB back, and a rebuild regenerates it
cargo clean -p mooloop-ui         # expensive; those seven binaries are the rest
```

**The first does not invalidate anything already built** -- verified here, a
`cargo test -p mooloop-ui --test fader_taper` straight afterwards reached
"Finished" in 0.56s and ran. It costs the next *changed* rebuild its
incremental cache, nothing more.

The hook does that reclaim itself when it fires with under 8 GB free, which is
what a `resume` or `compact` firing is good for -- by then a build has happened.
`MOOLOOP_WEB_RECLAIM_BELOW_GB` moves the threshold.

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

### Do not edit while a remote build is running

**A remote build that finishes *after* you edit a file will make the next run
ignore that edit.** `antibox` rsyncs with timestamps preserved, and Cargo
decides freshness by comparing a source's mtime against its build artifacts.
Edit at 20:55, let a build that started at 20:50 finish at 21:02, and the
remote now holds artifacts newer than your edited sources: the next run
rebuilds nothing that matters. For a `.slint` edit the build *script* is the
thing skipped, so `ui/main.slint` is silently the old one while your Rust is
the new one.

It presents as a compile error that makes no sense. On 2026-09-13 a struct
added to `main.slint` and used from `lib.rs` came back as `cannot find type
`PatternInfo` in this scope`, with the Slint build script's warnings visible
in the log -- **replayed from cache, which is what makes the log look like it
ran**. The same tree checked locally generated the struct and its setter
correctly. Twenty minutes went on hypotheses about Slint's struct export rules,
none of which were the answer.

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

sccache and incremental compilation cannot both be on -- `rustc` will not hand
sccache an incremental compilation unit -- so `antibox` picks one per run from
the profile. Measured on the box after a one-line edit in `mooloop-session`:

| Command | sccache | incremental |
| --- | --- | --- |
| `cargo test -p mooloop-session` | 31 s | **18 s** |
| `cargo test --workspace --exclude mooloop-ui` | 141 s | **43 s** |
| `cargo test --workspace` | 332 s | **118 s** |
| `cargo build --release -p mooloop-app` | **522 s** | 672 s |

Dev-profile `check`, `test` and `clippy` therefore get incremental
compilation, and release builds get sccache. `--incremental` and
`--no-incremental`, or `$MOOLOOP_INCREMENTAL=1|0`, override that.

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

`--dev-bin` does the same on the dev profile, to `./bin/mooloop-dev`. The
workspace's dev profile is tuned to be playable (`opt-level = 1` workspace
wide) and rebuilds in a fraction of release's time -- about 1 m 52 s on the
box against 522 s for `--release-bin`. **A dev binary is the default way to
hear a change**, and the one to hand Adam to listen to; keep `--release-bin`
for judging performance. Whether a dev build holds up under JACK with a real
song was the one open question, and it was closed on 2026-09-22 (MOO-168) on
Adam's instruction to treat outstanding listening passes as done with nothing
heard.

Both strip the binary, so it has no backtrace symbols; that is the right
default for listening and the wrong one for diagnosing a crash, which is what
`--keep-symbols` is for.

`scripts/mooloop-run` wraps the whole cycle into one command -- build on the
box, copy the binary down, run it here against JACK -- and falls back to a
capped local build if the box is unreachable. Its default is the dev profile,
which is the one to use:

```sh
scripts/mooloop-run              # dev profile
scripts/mooloop-run --release
scripts/mooloop-run --local      # never touch the box
```

### Keeping the box from filling up

Remote directories are keyed by the absolute path of the local checkout, and
every worktree ever built used to leave 10-20 GB behind forever; the box hit
436 GB and blocked every remote command. Each run now records the checkout it
came from, and `scripts/antibox --prune` deletes the caches whose checkout no
longer exists. A run that finds the box above 90% full says so and points at
it.

**That warning does not look like compiler output, and it is easy to grep
past.** It is one line beginning `antibox: warning --`, printed before the
build starts; the failure it predicts arrives minutes later as
`error: failed to write ... No space left on device`, which reads like a
compiler error and is not one. An agent checking a backgrounded run with
`grep -E '^error|^test result'` -- the habit the verification ladder above
encourages -- sees the consequence and not the cause. Read the first lines of a
remote run's log, not only the compiler-shaped ones.

**One task per worktree means one remote cache per worktree.** A session that
works through several tasks in a row, as `AGENTS.md` requires, leaves a
`mooloop-ui` target behind for each branch it has finished with, and they are
20-25 GB each. On 2026-09-12 six of them filled the box mid-run and `--prune`
freed 153 GB. `--prune` only reclaims a cache once its *local* worktree is
gone, so the moment to run it is just after `git worktree remove`, not when a
build fails.

## All Tests And Release Verification

This is the full integration suite. Run each line after the previous one has
finished; Cargo commands must not overlap on this workstation.

```sh
cargo test --workspace -j 2
cargo clippy --workspace --all-targets -j 2 -- -D warnings
cargo run -p mooloop-app --bin engine-selftest -j 2
MOOLOOP_AUTODRIVE=1 cargo run -p mooloop-app --bin mooloop -j 2
```

The last command exercises the app's automated smoke path.
`MOOLOOP_AUTODRIVE_RECORD=1` is its twin for recording (`audio-recording/05`):
a kick plays, a new sampler takes the master as its AUDIO input and records
one clip bar from its RECORD page, and the report says whether the page showed
the pre-roll and the take and whether the take became the sampler's sample as
one "Record Take" undo step. Give it a throwaway `MOOLOOP_CONFIG_DIR` -- the
take is written into that folder's `recordings/`. It drives the callbacks
in-process because the AUDIO row is a popup the MCP tools cannot click.

For UI work, the
useful visual check is a software-rendered snapshot of the real window:

```sh
MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm \
  cargo test -p mooloop-ui --test playlist_snapshot
magick /tmp/window.ppm /tmp/window.png
```

That is one of about fifty such snapshots — every source face and its pages,
the mixer, the modulation shelf, the preferences pages, before/after pairs
either side of a drag. Each is a test that always asserts and writes its image
only when its environment variable is set. List them with
`rg -o 'MOOLOOP_[A-Z_]+_SNAPSHOT' crates/mooloop-ui/tests | sort -u`.

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
be drawing.

**It does not reach inside a `PopupWindow`.** A `PickerChip` or a `BusPicker`
opens on click and its rows appear in the element tree with correct absolute
positions, so clicking one looks like it should work — and does nothing. The
popup closes and the value is unchanged, which is indistinguishable from a
callback that never fired, and it is an easy half hour to spend concluding
that a shipped widget is broken. It is not: `BusPicker` is the control test,
because it reports its choice *before* closing and fails here identically.
Verify a picker some other way — a snapshot test that sets the model directly,
or a unit test of the handler's session half — and use the live application
for the things it is uniquely good at, which is everything outside a popup. The engine connects to JACK on startup and usually reports one
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

## Working In A Cloud Container

An agent session on Claude Code's web runner gets a fresh Ubuntu container
that is neither the laptop nor the box: no `mold`, no audio or font
development headers, and an empty `target/`. Two things stop the build
outright before any code is compiled, and both are the container's rather
than the tree's.

**`mold` is not installed, and `.cargo/config.toml` pins it.** Every link
fails with `collect2: fatal error: cannot find 'ld'`, which names the wrong
thing and reads like a broken toolchain; `ld`, `ld.lld` and `ld.gold` are all
present and only mold is missing. Override the config's target rustflags from
the environment rather than editing the file -- `.github/workflows/ci.yml`
already does this with `RUSTFLAGS: ""`, and either form takes precedence over
`target.*.rustflags`:

```sh
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=lld"
```

**`jack-sys`'s build script panics when pkg-config cannot find JACK**, so
nothing compiles at all until the headers are installed. The CI job's
dependency list is the authoritative one; run `apt-get update` before it or
stale package lists 404 on part of it. A 2026-09-20 session built and tested
`mooloop-ui` with `libjack-jackd2-dev`, `libfontconfig-dev` and
`libxkbcommon-dev` alone, and `autoconf` and `nasm` were installed on a guess
and turned out to be unnecessary: `mp3lame-sys` builds LAME with plain gcc.

**A verification pass in a container hits two separate walls, and neither
error names itself.** Both were met on 2026-09-20 on four cores and 16 GB, and
they are not the same failure -- treating them as one sends you after the
wrong remedy.

**Memory, during `mooloop-ui`'s own `rustc`.** `cargo test --workspace` at
default parallelism ran four `rustc` at once and the `mooloop-ui` one died
with `signal: 9, SIGKILL`. Cargo reports that as `error: could not compile
mooloop-ui`, which reads like a broken build; a signal 9 with no diagnostic
above it is the OOM killer, not the compiler. **`-j 1` for anything that
compiles `mooloop-ui`** is the remedy: it is the crate that will not fit
beside three others. Splitting the pass -- workspace `--exclude mooloop-ui` at
full parallelism, then `mooloop-ui` alone at `-j 1` -- keeps most of the
speed.

**Disk, everywhere.** One `cargo test --workspace` took `target/` to 30 GB,
**15 GB of it `target/debug/incremental` alone**, and filled the session's
allowance. That one does not announce itself as a cargo error at all: what was
seen first was the harness losing a command's output entirely, because its own
temp files had nowhere to go. `df` is misleading here in the way this
document's header warns -- the allowance is spent while the machine looks
fine. So **export `CARGO_INCREMENTAL=0` for a verification pass** -- in a
container, and only there. `rm -rf target/debug/incremental` recovers the
space without touching the compiled artifacts, and works from inside an
already-wedged session because deletes still succeed while writes fail.

**That is the exact opposite of the laptop rule above, and both are right.**
The Cargo-limits section says not to turn incremental off, measured: on a
`.slint` edit it cost 4.58 GB against 3.42 GB and 2m01s against 41s. That is
an *edit-rebuild cycle*, where incremental is doing the job it exists for. A
container verification pass compiles each crate once from cold, so there is no
second build for the 15 GB to pay off in -- and the container's scarce
resource is disk, which the laptop has plenty of, while the laptop's scarce
resource is memory. **Note what that means: turning incremental off spends
memory to save disk**, and memory is the other wall on this list. It is
survivable here only in company with the `-j 1` above -- one 4.6 GB `rustc`
fits in 16 GB, four do not. Do not carry either setting to the other machine.

Even with it off, a full rung 4 only just fits, and it is worth knowing the
shape before starting one. The same worktree sat at **13 GB free** after a
workspace test, a workspace clippy and a `mooloop-ui` check -- and at **1.8 GB
free** once `cargo test -p mooloop-ui` and its clippy had built the test
profile's own dependency tree on top. That last pair is 11 GB and half an hour
(987 s and 1007 s at `-j 1`). Budget for it, or split rung 4 across two
sessions.

Everything else in this document still holds, `scripts/exit-code` included.
What does not carry over is the timing. Measured on four container cores that
day: `cargo check -p mooloop-ui` 4m50s with its dependencies already built;
`cargo test -p mooloop-ui --lib` 11m25s, which included compiling the test
profile's dependency tree; and
`cargo clippy -p mooloop-ui --all-targets -- -D warnings` 14m56s. That is a
verification pass, not an iteration loop -- decide what to run once, background
it, and do other work while it runs.

## Developing On macOS

The workspace builds and runs on a Mac, where the engine plays through Core
Audio instead of JACK (`docs/plans/archive/coreaudio-driver/`). The Xcode command-line
tools and `rustup` are all it needs -- Homebrew's `rustup` is keg-only, so put
`$(brew --prefix rustup)/bin` on `PATH` -- and `mold` is not used.

MIDI input comes from Core MIDI with nothing to set up: mooloop listens to
every source and logs each one as `listening to the MIDI input "<name>"`. If a
keyboard plays nothing, that log line is the first thing to look for, and Audio
MIDI Setup's MIDI Studio window shows whether macOS sees the device at all.

**Audio input needs the microphone permission, and macOS refuses it
silently.** mooloop opens the system default input device at startup and logs
either `listening to the audio input "<name>"` or `no audio input: <reason>`;
a refused permission is one of the reasons and names Privacy & Security. With
no input, the channel sidebar's AUDIO row simply lists no input source, which
is the same thing it does on a machine that has no input device.

The driver retries the open once a second, so granting the permission should
be picked up without a relaunch -- **that has not been tested**, because the
machine it was written on had already granted it, and macOS is known to cache
a TCC decision for the life of a process. If the row does not appear within a
second or two of granting it, restart mooloop and say so here.

**One shared build cache, at `~/.cache/cargo-target`, on the external SSD.**
`.cargo/config.toml` says a workstation wanting a cache shared across
worktrees sets `CARGO_TARGET_DIR` machine-locally, and on this Mac that is
done, in `~/.zshenv`. It was not optional: on 2026-09-20 the main checkout and
one task worktree held 26 GB and 29 GB of near-identical output, on a 228 GiB
internal disk with **6.2 GiB free**. Consolidating and moving it to the
external took the internal disk to **59 GiB free**, and a second worktree now
costs nothing -- measured at 0.38 s for a `cargo check` another checkout had
already done.

The configured path is a **symlink** into `/Volumes/Extended SSD`. That
indirection is load-bearing: the volume name contains a space, and an autoconf
build script handed a path with a space in it fails in ways that read as a
broken toolchain. `mp3lame-sys` is such a script and it is in this workspace.
Cargo does not canonicalise `CARGO_TARGET_DIR`, so `OUT_DIR` arrives
space-free. The guard in `~/.zshenv` tests the *volume* rather than the
symlink, so a detached drive leaves `CARGO_TARGET_DIR` unset and cargo falls
back to a per-project `target/`; naming a path under an unmounted
`/Volumes/<name>` would instead resolve on the boot disk and quietly fill the
disk the move exists to spare.

**It is on a USB 2.0 cable, and that costs more than it sounds.** Measured
2026-09-20: the drive is a SanDisk Extreme 2 TB plugged *directly* into a
USB 3.1 bus, and it still negotiates 480 Mb/s and writes at ~41 MB/s, against
the internal's ~904 MB/s. Not the hub, not the port, not the drive -- the
cable. An Apple iPad USB-C cable carries charge and USB 2 data only, with none
of the SuperSpeed pairs.

What that buys, measured on the same tree: touching `mooloop-engine/src/lib.rs`
and rebuilding `mooloop-app` took **3 m 29 s of wall clock for 23.6 s of CPU**
-- eleven per cent utilisation, so seven eighths of the build was waiting on
the cable. Judge any build time on this machine against that before concluding
something in the workspace got slower. A cable marked `SS` or `10Gbps` should
return it to roughly internal speed with nothing to change in `~/.zshenv`;
re-measure with `dd if=/dev/zero of="/Volumes/Extended SSD/.st" bs=4m count=200`.

**`cargo` reaches non-interactive shells through `~/.zshenv`, not `.zshrc`.**
Homebrew's `rustup` is keg-only, so its shims are never linked into
`/opt/homebrew/bin` -- and `rustup` itself resolves while `cargo` does not,
which reads as a broken toolchain rather than as a missing `PATH` entry. The
prepend was put in `~/.zshenv` on 2026-09-20 because that is the only startup
file zsh reads for every invocation: `.zshrc` opens with
`[[ $- == *i* ]] || return`, and `.zprofile` is login shells only, so a
`zsh -c "cargo ..."` -- which is how scripts, editors and coding agents run
things -- sees neither. It is machine-local and not in the repository; a fresh
clone on another Mac needs it made again.

The JACK adapter does not compile on a Mac, so an edit to it, or to anything
else behind `cfg(not(target_os = "macos"))`, goes unchecked there.
`scripts/linux-check` checks the Linux build from the Mac instead:

```sh
scripts/linux-check                     # cargo check -p mooloop-engine --all-targets
scripts/linux-check clippy -p mooloop-engine --all-targets -- -D warnings
```

It needs `rustup target add x86_64-unknown-linux-gnu` and `brew install zig`.
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

## Recordings

A take is written, as it records, into a **recordings folder** as a 32-bit
float stereo WAV named `<UTC date>-<time>-<channel>.wav`
(`mooloop_session::take::TakeRecorder`). The folder is the recorder's to be
told; the interface that arms takes (`audio-recording/05`) points it at
`recordings/` beside the settings file. A take that recorded nothing leaves no
file. Until step 04, nothing moves a take into a project or deletes an unused
one (step 06).

## Diagnostic Log

The app writes a levelled record of what it does to stderr: what it opened and
saved, every correction the repair pass applied, xruns, and any failure. A run
started from a terminal shows it without any setup.

```sh
MOOLOOP_LOG=debug cargo run -p mooloop-app --bin mooloop -j 2
```

`MOOLOOP_LOG` takes `error`, `warn`, `info` (the default), or `debug`. The
older `MOOLOOP_DEBUG=1` still works and now means `debug`.

Most problems are not reported from a terminal, so **Preferences → Developer →
Write a log file** mirrors everything, `debug` included, to
`$MOOLOOP_CONFIG_DIR/mooloop.log` (by default `~/.config/mooloop/mooloop.log`).
It appends across runs and rolls to `mooloop.log.1` past 4 MB. The preference
sticks, so it can be switched on before trying to reproduce something.

### The Output Went Missing

A saved output destination outlives the thing it names. Unplug the headphones,
change an ALSA profile, or restart the audio server and have it rename its
nodes, and the pair recorded in `settings.toml` matches nothing in the graph.

Connecting to nothing is the one failure with no symptom — the engine runs,
the meters move, the transport rolls, and there is silence. So under JACK the
output follows Adam's rule (2026-09-22):

- **Stay on the most recently picked output that is there.** Preferences
  remembers the last eight outputs picked (`earlier-outputs` in the driver's
  section of `settings.toml`, beside `output-port-l`/`-r`). At launch, and
  whenever the outputs are connected to nothing, mooloop connects to the most
  recent of them whose ports are all in the graph.
- **Never move an output that is connected.** A device appearing while
  mooloop plays somewhere is left alone, even one picked more recently: plug
  in a USB interface while the speakers play, and the speakers keep playing.
  "Connected" means connected to anything, including a link made by hand in
  a patchbay.
- **Something over nothing.** With no remembered output there, any working
  stereo destination is taken, unranked -- the graph's first that is not
  mooloop itself. Preferring speakers over HDMI would be a guess about a
  machine the engine cannot see.

So if headphones on the same card replace the speakers' ports when plugged
in, the speakers' ports go, the output is connected to nothing, and it moves
to the headphones if they are the more recent pick. A second device that
simply appears changes nothing.

```text
warn  audio  the saved audio output "..." is not available; connected to
             "alsa_output...HiFi__Speaker__sink:playback_FL" instead.
             Preferences -> Audio picks a different one
info  audio  the audio output "..." is not connected; playing through "..."
```

The check runs on the control thread, from the engine's event poll, half a
second after the port graph last changed and once a second otherwise. It
used to run inside JACK's port-registration callback, which pipewire-jack
does not let wait on its own graph requests, and it only ever reconnected
the one remembered pair -- after a fallback, the fallback. Turning
auto-reconnect off in Preferences stops everything but the choice at launch.

Moving the output connects the new pair before letting go of the old one, so
choosing one in Preferences that will not connect leaves the current one
playing. Core Audio still returns to the picked device when it comes back,
even while the system default is playing; it has not been moved to this rule.

To see the state directly: `pw-link -l | grep mooloop` lists the links, and no
output at all means the outputs are connected to nothing.

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
audio thread in time. Nothing about the project will help that.

The xrun count is a count, not a flag. Several arrive between two blocks when a
machine stalls, and reporting only that the number changed is how a run of
audible dropouts used to read as a single line.

The first thing to check when late wake-ups appear is whether the callback is
realtime at all. It is warned about once per run:

```text
warn  audio  the audio callback is running on an ordinary time-shared thread,
             not a realtime one; ...
```

Under PipeWire this is usually `rtkit` having demoted every realtime thread on
the machine after its canary starved — which a heavy local build is enough to
cause, and which nothing undoes automatically. `journalctl -b -u rtkit-daemon`
shows it as "Demoting known real-time threads", and
`scripts/procs --threads data-loop` confirms the current policy: it prints
each matching thread's policy and realtime priority beside the process that
owns it, and agrees with `chrt -p` because it reads the same two stat fields.

This file carried `chrt -p $(pgrep -f data-loop)` for that check until
2026-09-15, and it could never have worked. `data-loop` is a *thread* inside
`pipewire`, and a thread name is not in any command line, so `pgrep -f` never
matched the thread -- it matched the shell asking the question, whose command
line contains the pattern. `chrt` then reported that shell's SCHED_OTHER,
which is exactly the symptom under investigation, arriving as the answer.
`systemctl --user restart pipewire pipewire-pulse wireplumber` asks again.
Putting the user in the `pipewire` group is the durable fix: Fedora's
`/etc/security/limits.d/25-pw-rlimits.conf` grants that group `rtprio 70`
directly, so PipeWire stops depending on rtkit's judgement.

A song that cannot be saved is written to `~/.config/mooloop/quarantine/`
anyway, with a `.txt` beside it holding the same explanation the dialog showed.
Assets are referenced rather than embedded, so this is fast and the file is
small. Open one with `toml` in hand rather than the app: loading it through the
app repairs it, which is what destroys the evidence.

Nothing here may be called from the audio thread; see
`crates/mooloop-core/src/log.rs`.

## Commit, Merge, And Tidy Up

Commit small, buildable changes from the task worktree. If your model+harness
pair has no row in `CONTRIBUTORS.md`, add one; an existing row needs nothing
per commit.

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
fast-forward it into a clean, current `main`. Then tag the exact `main` commit
and push the branch before the tag:

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
attaches them to a GitHub Release. Because the app is not notarized, macOS
refuses the first double-click. Right-click > Open once, or run
`xattr -dr com.apple.quarantine Mooloop.app`. Every job also runs, without
publishing, from **Run workflow** (`workflow_dispatch`), which is how to check a
workflow change before tagging.
The release workflow does the distribution build against Ubuntu 20.04 for a
glibc 2.31 baseline; the local release build is a useful check, not a
substitute for those artifacts.

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
branch:

```sh
git worktree remove ../mooloop-worktrees/<short-name>
git branch -d <type>/<short-name>
```

If a worktree directory was already removed outside Git, clear only Git's
stale administrative record:

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
`scripts/procs`, never with `pgrep -f`/`pkill -f`, which match the command
line you are typing and, in the `pkill` case, kill the agent's own shell:

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
