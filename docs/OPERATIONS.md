# Building Mooloop: Cargo And Git

This is the short version of how we operate the repository. The machine is
usually short on memory while linking, not CPU: run Cargo commands one at a
time, and use `-j 2` for whole-workspace work. The committed default is `-j 3`
for smaller commands; do not turn it up.

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

# Run the application (requires JACK or PipeWire's JACK layer).
cargo run -p mooloop-app --bin mooloop -j 2

# Optimized build, suitable for a local performance or packaging check.
cargo build --release -p mooloop-app -j 2

# Format check for CI; omit --check to apply formatting.
cargo fmt --check

# Lint the whole workspace exactly as CI does.
cargo clippy --workspace --all-targets -j 2 -- -D warnings
```

For a narrow change, test the crate you touched. `mooloop-ui` is the heavy
one, so retain its explicit job cap:

```sh
cargo test -p mooloop-dsp -j 2
cargo test -p mooloop-engine -j 2
cargo test -p mooloop-ui -j 2
```

## All Tests And Release Verification

This is the full integration suite. Run each line after the previous one has
finished; Cargo commands must not overlap on this workstation.

```sh
cargo test --workspace -j 2
cargo clippy --workspace --all-targets -j 2 -- -D warnings
cargo run -p mooloop-app --bin engine-selftest -j 2
MOOLOOP_AUTODRIVE=1 cargo run -p mooloop-app --bin mooloop -j 2
```

The last command exercises the app's automated smoke path. For UI work, a
software-rendered sheet of the real widgets is the useful visual check --
the mockup tool, loaded with a saved layout:

```sh
SLINT_BACKEND=winit-software MOOLOOP_MOCKUP_SNAPSHOT=/tmp/widgets.ppm \
  MOOLOOP_MOCKUP_LAYOUT=crates/mooloop-ui/tests/fixtures/widget-sheet.toml \
  MOOLOOP_MOCKUP_SIZE=1400x900 cargo run -p mooloop-ui --example mockup
magick /tmp/widgets.ppm /tmp/widgets.png
```

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

A song that cannot be saved is written to `~/.config/mooloop/quarantine/`
anyway, with a `.txt` beside it holding the same explanation the dialog showed.
Assets are referenced rather than embedded, so this is fast and the file is
small. Open one with `toml` in hand rather than the app: loading it through the
app repairs it, which is what destroys the evidence.

Nothing here may be called from the audio thread; see
`crates/mooloop-core/src/log.rs`.

## Commit, Merge, And Tidy Up

Commit small, buildable changes from the task worktree. Update your entry in
`CONTRIBUTORS.md` before each commit.

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
`.deb`, `.rpm`, and AppImage packages and attaches them to a GitHub Release.
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

Finally, clean untracked build output only after looking at it:

```sh
git clean -ndX
git clean -fdX
```

The first command is the dry run. The second removes ignored files, including
unwanted `target/` output, but leaves untracked non-ignored files alone.
