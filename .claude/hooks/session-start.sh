#!/usr/bin/env bash
# SessionStart hook for Claude Code on the web.
#
# A fresh web container is a bare Ubuntu image with a Rust toolchain and
# nothing else: no audio, graphics or font development headers, and no `mold`,
# which `.cargo/config.toml` pins as the linker for this target. Every build
# fails at `build.rs` or at link time until they are installed, and an agent
# that does not know that spends its first twenty minutes discovering it.
#
# It also has 15 GB of RAM, four cores and **no swap**, which is the part that
# differs from every other machine this project builds on. Adam's laptop has
# zram, CI runners have a swapfile; here a spike over the limit is an OOM kill,
# not a slowdown. Two `mooloop-ui` rustc processes reach about 14 GB together,
# so the repository's `jobs = 3` -- chosen for a 16 GB laptop with swap -- is
# not survivable here. See docs/OPERATIONS.md, "Claude Code on the web".
#
# Kept synchronous deliberately: the first thing a session does is usually a
# Cargo command, and an async install loses the race.
set -euo pipefail

# Everything below is about this container. On Adam's machine the packages are
# installed, the limits are different, and scripts/cargo-capped owns the memory
# bound; do not let this hook touch it.
if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"

# The build dependencies, kept identical to the list .github/workflows/ci.yml
# installs, plus mold. If a build starts failing here for a missing header,
# fix it in both places -- CI is the reference and this is the copy.
PACKAGES=(
  pkg-config
  # Not a build dependency: `/usr/bin/time -v` is how a memory question gets
  # answered here, and the image does not ship it. Costs nothing to install.
  time
  libjack-jackd2-dev
  libasound2-dev
  libgl1-mesa-dev
  libegl1-mesa-dev
  libxkbcommon-dev
  libxcb1-dev
  libx11-dev
  libxcursor-dev
  libxrandr-dev
  libxi-dev
  libwayland-dev
  libfontconfig1-dev
  mold
)

missing=()
for pkg in "${PACKAGES[@]}"; do
  dpkg -s "$pkg" >/dev/null 2>&1 || missing+=("$pkg")
done

if [ "${#missing[@]}" -gt 0 ]; then
  echo "session-start: installing ${#missing[@]} package(s): ${missing[*]}"
  export DEBIAN_FRONTEND=noninteractive
  SUDO=""
  [ "$(id -u)" -eq 0 ] || SUDO="sudo"
  $SUDO apt-get update -qq
  $SUDO apt-get install -y -qq --no-install-recommends "${missing[@]}"
else
  echo "session-start: build packages already present"
fi

# --- Cargo environment for this container -----------------------------------

# Appended, never truncated: the file is shared with anything else that writes
# session environment, and this hook re-fires on resume and compact. Repeating
# an export is harmless; dropping someone else's is not.
env_line() {
  [ -n "${CLAUDE_ENV_FILE:-}" ] && echo "$1" >> "$CLAUDE_ENV_FILE"
  return 0
}

# One rustc at a time. The peak that matters is a single `mooloop-ui` process
# on the module slint-build generates from ui/main.slint: 5.8 GB measured in
# this container, and `cargo test -p mooloop-ui` has seven such units because
# every test binary compiles its own copy of it. One fits with nine to spare;
# two do not, and there is no swap to make the overshoot survivable. Small crates pay for this in
# wall clock, so pass `-j 4` explicitly for anything that is not mooloop-ui:
#
#   cargo test -p mooloop-dsp -j 4
#
# Do not raise the default to "fix" a slow workspace run. The workspace run is
# exactly the command that gets killed.
env_line 'export CARGO_BUILD_JOBS=1'

# CARGO_INCREMENTAL is deliberately left alone. Turning it off is the obvious
# way to save disk and it costs memory instead (4.58 GB against 3.42 GB on the
# same edit, measured for docs/OPERATIONS.md); memory is the scarcer resource
# here. The incremental directory is reclaimed below when disk gets tight.

if command -v mold >/dev/null 2>&1; then
  MOLD_STATE="mold $(mold --version 2>/dev/null | awk '{print $2}')"
else
  # Survive an apt failure rather than failing every build with a linker error
  # nobody will connect to this hook. An explicit RUSTFLAGS overrides the
  # target.*.rustflags in .cargo/config.toml, which is what CI does too.
  env_line 'export RUSTFLAGS=""'
  MOLD_STATE="mold MISSING -- RUSTFLAGS cleared, builds use the default linker"
fi

# AGENTS.md requires this once per clone, and a web container is a new clone
# every time. Without it the pre-commit hook that keeps non-Markdown commits
# off main is not running at all.
git -C "$PROJECT_DIR" config core.hooksPath .githooks

# --- Disk ------------------------------------------------------------------
#
# The session's writable allowance is finite and a debug `target/` for this
# workspace is a large fraction of it. This hook re-fires on resume and
# compact, by which point a build has happened, so use those firings to
# reclaim the cheapest thing first: incremental state, which a rebuild
# regenerates.
avail_gb() { df -BG --output=avail "$1" | tail -1 | tr -dc '0-9'; }

# Measured 2026-09-19 after one `cargo test -p mooloop-ui --no-run`: target/
# was 22G, of which 13G is deps/ (seven test binaries at ~363M each, plus the
# 679M rlib) and 8.4G is incremental state. The incremental half is the half
# a rebuild regenerates, so it is what this reclaims.
RECLAIM_BELOW_GB="${MOOLOOP_WEB_RECLAIM_BELOW_GB:-8}"
DISK_FREE="$(avail_gb "$PROJECT_DIR")"
if [ "${DISK_FREE:-999}" -lt "$RECLAIM_BELOW_GB" ] && [ -d "$PROJECT_DIR/target/debug/incremental" ]; then
  echo "session-start: ${DISK_FREE}G free, dropping target/debug/incremental"
  rm -rf "$PROJECT_DIR/target/debug/incremental"
  DISK_FREE="$(avail_gb "$PROJECT_DIR")"
fi

MEM_GB="$(free -g | awk '/^Mem:/ {print $2}')"
SWAP_GB="$(free -g | awk '/^Swap:/ {print $2}')"

cat <<EOF
session-start: ready.
  build deps: installed ($MOLD_STATE)
  machine:    ${MEM_GB}G RAM, ${SWAP_GB}G swap, $(nproc) cores, ${DISK_FREE}G disk free
  cargo:      CARGO_BUILD_JOBS=1 for this container, overriding the
              repository's jobs = 3. One mooloop-ui test rustc peaks at 8.5G of
              ${MEM_GB}G and there are seven of them; with ${SWAP_GB}G swap a
              second concurrent one is an OOM kill, not a slowdown. Pass -j 4
              explicitly for crates other than mooloop-ui. scripts/cargo-capped
              is a no-op here -- no systemd to put the run in a cgroup.
  disk:       a built target/ is ~22G of the allowance. 'rm -rf
              target/debug/incremental' reclaims about 9G of that and a rebuild
              regenerates it; 'cargo clean -p mooloop-ui' is the expensive one.
EOF
