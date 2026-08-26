# Agent Operating Contract

## Git workflow: mandatory

`main` is read/merge-only. Do not edit any repository-tracked file in its
checkout, including documentation and metadata, unless Adam explicitly
instructs you to do so.

Before every task, run `git status --short --branch`.

- If the tree has unrelated changes, stop and ask Adam whether to commit,
  discard, or split them before starting another task. Never stash and forget
  them.
- If you are on `main`, create a sibling task worktree before editing:

  ```sh
  git worktree add ../mooloop-worktrees/<branch-name> -b <type>/<slug> main
  ```

  Use `feat/`, `fix/`, `refactor/`, `chore/`, or `spike/` prefixes. Do all
  edits, builds, and tests in that worktree. One task uses one branch and one
  worktree.
- Check CodeGraph and index or init as needed without asking. Adam wants you to use CodeGraph
- Commit small, buildable changes. Do not use `--no-verify`, rewrite shared
  history, or commit generated output, `target/`, or secrets.
- Before every commit, update your model+harness entry in `CONTRIBUTORS.md`.
- Finish with a clean worktree, proportional verification, and a fast-forward
  merge to `main`. Do not merge, force-push, reset, or delete a worktree with
  uncommitted or unmerged work without Adam's explicit confirmation.

The tracked pre-commit hook rejects ordinary commits on `main`; activate it
once per clone with `git config core.hooksPath .githooks`.

## Task context

Source and tests are the truth for current behavior. Read only the documents
that affect the decision at hand:

| Task | Required context |
| --- | --- |
| Product or architecture decision | `docs/PRODUCT.md`, then the relevant architecture/design document |
| Open-ended priority or scope choice | `docs/FOCUS.md` and `docs/ROADMAP.md` |
| Broad existing user surface or known gap | `docs/CURRENT.md` |
| UI layout, controls, or interaction | `docs/UI_DESIGN.md` |
| Retained-audio buffer work | `docs/BUFFER_ENGINE.md` |
| Audio-engine contract work | `docs/AUDIO_ARCHITECTURE.md` |

Current explicit user feedback and purpose-built UI designs outrank these
documents.

## Slint

This project pins Slint `1.17.1`. Before editing `.slint`, `slint::` Rust API,
or `slint-build`, consult the version-matched documentation:
`https://releases.slint.dev/1.17.1/docs/slint/`. If the pinned version changes,
use the matching release URL instead of relying on latest-version knowledge.

## Verification and operations

Choose the smallest verification that covers the change: targeted validation
for documentation, the affected Rust test/check for isolated code, and the
specific software-rendered UI snapshot for UI work. Do not run Cargo commands
concurrently. Read [docs/AGENT_OPERATIONS.md](docs/AGENT_OPERATIONS.md) before
running Cargo, UI snapshots, or the live application; it contains this
machine's memory limits and rendering procedures.
