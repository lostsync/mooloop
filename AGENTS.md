## Project definition

Read `docs/PRODUCT.md` before making product or architecture decisions.
`docs/CURRENT.md` records implemented behavior, `docs/ROADMAP.md` orders future
work, `docs/FOCUS.md` names what we are working on next and what is
deliberately deferred, and `docs/BUFFER_ENGINE.md` defines the buffer-centric
hypothesis. Read
`docs/UI_DESIGN.md` before changing UI layout or controls; its agent checklist
is the acceptance contract for interface work. Current explicit user feedback
and current purpose-built UI designs outrank all of these documents.

## Slint version

This project pins Slint `1.17.1` (see the `slint`/`slint-build` entries in
`Cargo.toml` and `Cargo.lock`). Before writing or editing `.slint` files, the
`slint::` Rust API, or `slint-build`, fetch the version-matched docs rather
than relying on general/latest knowledge — Slint's language and widget API
have changed across versions, and a mismatch reads as plausible but wrong
code (deprecated properties, renamed callbacks, layout syntax that no longer
compiles).

Docs for this exact version live at
`https://releases.slint.dev/1.17.1/docs/slint/` (currently redirects to
`https://docs.slint.dev/latest/docs/slint/`, since 1.17.1 is the current
release). If `Cargo.toml`'s pinned Slint version ever changes, update this
URL to match — `https://releases.slint.dev/<version>/docs/slint/` is the
general pattern for any archived version.

# Working agreement for AI coding agents (Claude Code, Codex, opencode)

Adam works alone and is not disciplined about git by default — he leans on
Ctrl+Z. That's fine for him, but it's not fine for us. When multiple agents
touch this repo, sloppy git hygiene causes silent data loss and merge
disasters. These rules exist to make git the safety net, not a landmine.
Every agent working in this repo follows this file, no exceptions.

## The core rule: never switch gears with a dirty tree

Before starting *any* new task, run `git status`. If the working tree is
dirty and the changes are not part of the task you're about to start:

- Do NOT just start editing on top of it.
- Do NOT stash it and forget about it.
- Either finish and commit what's there, or stop and ask Adam what he wants
  done with it (commit / discard / put in its own branch).

Leaving uncommitted work behind while moving on to something else is the
exact failure mode this file exists to prevent.

### Documentation exception

An untracked Markdown file directly under `docs/` may be treated as an
intentional project note: inspect it, then add and commit it with the current
documentation work without asking for separate confirmation. This exception
does not apply to modified files, files outside `docs/`, generated artifacts,
or any non-Markdown content.

## One task = one branch = one worktree

Never do a task's work directly on `main`, and never do two unrelated
things on the same branch. For every non-trivial task (bug fix, feature,
refactor):

1. Create a worktree off `main`, in a sibling directory, with a new branch:

   ```
   git worktree add ../mooloop-worktrees/<branch-name> -b <type>/<slug> main
   ```

   Branch prefixes: `feat/`, `fix/`, `refactor/`, `chore/`, `spike/`.
   Example: `git worktree add ../mooloop-worktrees/fix-sampler-clip -b fix/sampler-clip main`

2. Do all editing, building, and testing inside that worktree's directory.
   Never edit files in one worktree while `cd`'d into another.

3. This is what makes parallelism safe: Claude, Codex, and opencode can each
   own a different worktree/branch at the same time without stepping on each
   other's uncommitted state. If you're about to start work and another
   agent's worktree already exists for a related branch, say so instead of
   silently working around it.

Claude Code specifically: prefer the Agent tool's `isolation: "worktree"`
option for delegated sub-tasks instead of hand-rolling `git worktree`
commands, when spawning a subagent for isolated work.

## Commit discipline

- Commit early and often within a branch — small, atomic, buildable
  commits, not one giant diff at the end.
- Write commit messages in the imperative mood that explain *why*, not just
  *what* (the diff already shows what).
- Never commit secrets, generated artifacts, or `target/` output.
- Never use `--no-verify`, `--amend` on already-shared commits, or rewrite
  history on a branch other agents might be using, without asking first.

## Signing in

Adam wants a record of which model+harness combinations have worked on this
project, so he can credit contributors later. Before considering *any* task
done where you made a commit, add or update your entry in
[CONTRIBUTORS.md](./CONTRIBUTORS.md) per the instructions at the top of that
file, then include the update in the same commit (or a trailing one) as the
rest of your work.

## Finishing a task

- Working tree must be clean before you consider a task done. Run
  `git status` as a final check.
- Verify changes before merging, using the smallest check that covers the
  changed behavior:
  - Documentation, metadata, instructions, and static assets need only
    targeted validation; do not compile the workspace for them.
  - For isolated Rust changes, run the affected package's test or check
    (`cargo test -p <package>` or a narrower named test). Scope Clippy the
    same way when lint coverage is relevant.
  - For UI changes, run the specific UI test or software-rendered snapshot
    that exercises the changed surface. Do not run every UI integration-test
    binary merely because the change is in `mooloop-ui`.
  - Run workspace-wide tests/Clippy only for cross-crate API or dependency
    changes, release/integration work, or when explicitly requested.
- Never run Cargo build, test, or Clippy commands concurrently on this
  workstation. Parallel rustc/link processes already run within each command.
- Cargo runs on this machine are limited by **memory, not CPU**. Adam is
  usually using the laptop while an agent works, and it has 15 GB with
  several GB already in use. What locks it up is concurrent *linking*:
  `cargo test -p mooloop-ui` produces seven binaries, each around 217 MB even
  with the dev profile capped (measured; they were ~450 MB before).

  Two things follow, and the first matters far more than the second:

  1. Keep dev debug info capped. `[profile.dev]` in the workspace
     `Cargo.toml` sets `debug = "line-tables-only"` and
     `split-debuginfo = "unpacked"`. Do not raise these to chase a debugger
     session without putting them back. A per-package override is not
     enough — it only thins that package's own code, while every dependency
     rlib linked into the binary keeps full DWARF. This is worth about 2.2x,
     not the order of magnitude it looks like it should be: most of a
     mooloop-ui binary is Slint's generated code, not debug info.
  2. Cap parallelism on the heavy crates:

     ```
     cargo test -p mooloop-ui -j 2
     ```

     `.cargo/config.toml` sets `jobs = 3` as the floor for anything that
     forgets an explicit `-j`. Eight, six, and four have each locked this
     machine up; three has held. Prefer a single named target
     (`--test mixer_snapshot`) over building every test binary in a package
     when you only need one.

  `nice` does **not** help here and is not a substitute for either: it
  adjusts CPU scheduling priority, while memory allocation and page reclaim
  ignore it entirely. This composes with the no-concurrent-commands rule
  above rather than replacing it.
- Merge the branch back into `main` with a fast-forward (`git merge
  --ff-only <branch>` from a `main` checkout) so history stays a single
  straight line. If `main` has moved on and a fast-forward isn't possible,
  rebase the branch onto `main` first, then fast-forward. Then clean up:

  ```
  git worktree remove ../mooloop-worktrees/<branch-name>
  git branch -d <type>/<slug>
  ```

- Don't leave stale worktrees or merged branches lying around. `git
  worktree list` and `git branch --merged main` should stay tidy.

## Never do without explicit confirmation

`push --force`, `reset --hard`, rewriting published history, deleting a
branch/worktree that has unmerged or uncommitted work, or merging into
`main` without the verification appropriate to the change passing.

## If Adam hands you a dirty tree right now

Don't lecture him — just apply the rule above: figure out what the
uncommitted changes are, get them into a proper branch/commit (asking if
their intent is unclear), and only then start new work in its own
worktree.

## Checking the UI / taking screenshots

**Prefer headless software rendering.** It needs no compositor, no focus, no
window, and it keeps working while the screen is locked. It is also
deterministic and pixel-exact, so the same command can back a visual check and
a test assertion. Reach for the live app only when you actually need the
running instrument — real audio, JACK, or hand-driven interaction.

Slint's default GPU backend cannot do this: `take_snapshot` fails with "not
supported by this FemtoVG backend". Force the software renderer with
`SLINT_BACKEND=winit-software`.

- **Every control, on its own:**
  ```
  SLINT_BACKEND=winit-software MOOLOOP_GALLERY_SNAPSHOT=/tmp/gallery.ppm \
    MOOLOOP_GALLERY_SIZE=1000x1800 cargo run -p mooloop-ui --example control-gallery
  ```
  `MOOLOOP_GALLERY_SIZE` exists because the gallery is taller than a window;
  without it you capture only the top.
- **The real `MainWindow`:** the playlist snapshot test renders it headlessly
  and will dump what it rendered:
  ```
  MOOLOOP_PLAYLIST_SNAPSHOT=/tmp/window.ppm cargo test -p mooloop-ui --test playlist_snapshot
  ```
  This is also how to find pixel coordinates when a snapshot assertion moves:
  render, probe the pixels, then update the constants.
- **Convert for viewing:** `magick /tmp/whatever.ppm /tmp/whatever.png`

### Running the live app

Adam runs Hyprland and is often using the machine while an agent works. There
is a dedicated headless Wayland output named `agent` (workspace `agent`, bound
to it) so the GUI never appears on his real screens or steals focus. Use it
instead of whatever workspace happens to be active.

- **Launch the app on it:**
  `hyprctl dispatch exec '[workspace name:agent] <command>'`
  Two gotchas that will silently break this:
  - The `name:` prefix is required — bare `agent` gets misparsed and the
    window lands on Adam's active workspace instead.
  - Do **not** add `silent`. `silent` stops the *headless* monitor itself
    from switching to the `agent` workspace, so the window exists but
    nothing gets composited into it — screenshots come back blank. This is
    the opposite of normal Hyprland advice (`silent` is usually what you
    want) but here it's safe and necessary: the `agent` monitor is
    headless, so "switching to it" is never visible to Adam on a real
    screen.
- **Screenshot it:**
  `grim -o agent /tmp/whatever.png`
- **Find/inspect the window:**
  `hyprctl clients -j | jq '.[] | select(.workspace.name=="agent")'`
- **Close it when done:**
  `hyprctl dispatch closewindow address:<addr>` (get `<addr>` from
  `hyprctl clients -j`), or just kill the process.

**If `grim` returns only wallpaper while the window is mapped and the process
is alive, the lock screen is engaged.** Nothing is broken and the app is fine;
the compositor just is not painting that output. Don't debug it, and don't go
recreating the output — switch to software rendering above, which is immune.

If you need to click or type into the window (`ydotool`, no `wtype`
installed), know that input goes wherever keyboard/pointer focus currently
is — briefly focusing the agent window will steal Adam's input focus even
though nothing changes on screen. Keep such interactions short and don't
leave the agent workspace focused when you're done.

If the `agent` output is genuinely missing (e.g. after a compositor crash
before a full re-login), recreate it: `hyprctl output create headless agent`.
