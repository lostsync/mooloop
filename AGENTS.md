# Agent Operating Contract

The shared workflow contract for every agent working in this repository
(Claude Code, Codex, opencode, and whatever else is in rotation). Read it
before starting work; `CLAUDE.md` points here rather than repeating it.

## Git workflow: mandatory

`main` is read/merge-only. Do not edit any repository-tracked file in its
checkout, including documentation and metadata, unless Adam explicitly
instructs you to do so.

Before every task, run `git status --short --branch`.

- If the tree has unrelated changes, stop and ask Adam whether to commit,
  discard, or split them before starting another task. Never stash and forget
  them. An untracked Markdown file under `docs/` is the standing exception:
  commit it alongside doc work without asking.
- If you are on `main`, create a sibling task worktree before editing:

  ```sh
  git worktree add ../mooloop-worktrees/<branch-name> -b <type>/<slug> main
  ```

  Use `feat/`, `fix/`, `refactor/`, `chore/`, or `spike/` prefixes. Do all
  edits, builds, and tests in that worktree. One task uses one branch and one
  worktree.
- Commit small, buildable changes. Do not use `--no-verify`, rewrite shared
  history, or commit generated output, `target/`, or secrets.
- Before every commit, update your model+harness entry in `CONTRIBUTORS.md`.
  It is a roster: bump `Last seen` and `Sessions`, and keep `Notes` to the
  one-line summary the template asks for. Detail belongs in the commit
  message and, for a narrative, `docs/JOURNAL.md`.
- Finish with a clean worktree, proportional verification, and a fast-forward
  merge to `main`. Do not merge, force-push, reset, or delete a worktree with
  uncommitted or unmerged work without Adam's explicit confirmation.

The tracked pre-commit hook rejects ordinary commits on `main`; activate it
once per clone with `git config core.hooksPath .githooks`.

## CodeGraph

This project has a CodeGraph MCP server (`codegraph_*` tools) indexing every
symbol and edge. Adam wants it used. Check it and index or init as needed
without asking. Prefer it over grep for structural questions — where a symbol
is defined, what calls it, what an edit would break — and keep grep for
literal text. `.cursor/rules/codegraph.mdc` has the tool-by-question table.

## Task context

Source and tests are the truth for current behavior. Read only the documents
that affect the decision at hand.

| Task | Required context |
| --- | --- |
| What is being built next, and in what order | `docs/FOCUS.md`, then the matching `docs/plans/<name>/00-status.md` |
| Product or architecture decision | `docs/PRODUCT.md`, then the relevant architecture/design document |
| Open-ended priority or scope choice | `docs/FOCUS.md` and `docs/ROADMAP.md` |
| Broad existing user surface or known gap | `docs/CURRENT.md` |
| UI layout, controls, or interaction | `docs/UI_DESIGN.md` |
| A new shortcut, menu row, or command surface | `docs/ACTIONS.md` |
| Modulation sources, routes, or destination policy | `docs/MODULATOR_SYSTEM_SPEC.md` |
| Retained-audio buffer work | `docs/BUFFER_ENGINE.md` |
| Audio-engine contract work | `docs/AUDIO_ARCHITECTURE.md` |
| Extracting or publishing a reusable DSP unit | `docs/COMPOSABLE_DEVICE_UNITS.md` |
| Gain, level, or metering work | `docs/GAIN_STRUCTURE.md` |
| Save/load, migration, or a new persisted field | `docs/PROJECT_FORMAT.md` |
| Adding a limit to anything a user can create | `docs/CAPACITY_POLICY.md` |
| Reaching for a widget that may already exist | `docs/WIDGET_INVENTORY.md` |

Current explicit user feedback and purpose-built UI designs outrank these
documents.

Active work orders live in `docs/plans/<name>/`, numbered and worked in
order; `00-status.md` says what has landed. Update that status when a step
lands. Completed plan directories move to `docs/plans/archive/`.

`docs/README.md` indexes every document and states its one job, for anything
this table does not cover.

## Documentation is part of the change

`docs/CURRENT.md` describes the application as it exists. A change that adds,
removes, or alters user-visible behavior updates it in the same commit; so
does a change that invalidates a fact stated in any other document. Leaving
a document to be corrected later is how it stops being trusted.

## Slint

This project pins Slint `1.17.1`. Before editing `.slint`, `slint::` Rust API,
or `slint-build`, consult the version-matched documentation:
`https://releases.slint.dev/1.17.1/docs/slint/`. If the pinned version changes,
use the matching release URL instead of relying on latest-version knowledge.

Do not reach for `cargo build` to find out whether a `.slint` edit is valid or
what it looks like. `scripts/slint-sketch` type-checks a scratch `.slint`
against the real widgets in about 0.05s and screenshots it in about 0.2s,
where `cargo build -p mooloop-ui` is about four minutes for any edit at all.
Iterate there, then build once. It needs `slint-viewer` installed locally; it
is deliberately not a workspace dependency, and the build never refers to it.
See `docs/AGENT_OPERATIONS.md`.

## Verification and operations

Choose the smallest verification that covers the change: targeted validation
for documentation, the affected Rust test/check for isolated code, and the
specific software-rendered UI snapshot for UI work. Do not run Cargo commands
concurrently. Read [docs/AGENT_OPERATIONS.md](docs/AGENT_OPERATIONS.md) before
running Cargo, UI snapshots, or the live application; it contains this
machine's memory limits and rendering procedures; heavier runs belong on the
remote build box via `scripts/antibox`.
