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
| Which plans are live, and what state each is in | `docs/plans/README.md` |
| Broad existing user surface or known gap | `docs/CURRENT.md` |
| UI layout, controls, or interaction | `docs/UI_DESIGN.md` |
| Retained-audio buffer work | `docs/BUFFER_ENGINE.md` |
| Audio-engine contract work | `docs/AUDIO_ARCHITECTURE.md` |
| Gain, level, or metering work | `docs/GAIN_STRUCTURE.md` |

Current explicit user feedback and purpose-built UI designs outrank these
documents.

## Slint

This project pins Slint `1.17.1`. Before editing `.slint`, `slint::` Rust API,
or `slint-build`, consult the version-matched documentation:
`https://releases.slint.dev/1.17.1/docs/slint/`, and
`https://docs.rs/i-slint-backend-testing/1.17.1/` for the `ElementHandle` API
the UI tests and the MCP server both drive. If the pinned version changes, use
the matching release URL instead of relying on latest-version knowledge.

To see what the real interface does rather than what the source implies, run
`scripts/mooloop-mcp`: it starts the application with Slint's embedded MCP
server, whose tools read the live element tree and click, type, drag, and
screenshot it. `docs/AGENT_OPERATIONS.md` has the details.

Do not reach for `cargo build` to find out whether a `.slint` edit is valid or
what it looks like. `scripts/slint-sketch` type-checks a scratch `.slint`
against the real widgets in about 0.05s and screenshots it in about 0.2s,
where `cargo build -p mooloop-ui` is about four minutes for any edit at all.
Iterate there, then build once. It needs `slint-viewer` installed locally; it
is deliberately not a workspace dependency, and the build never refers to it.
See `docs/AGENT_OPERATIONS.md`.

## Verification and operations

Do not run Cargo commands concurrently. Read
[docs/AGENT_OPERATIONS.md](docs/AGENT_OPERATIONS.md) before running Cargo, UI
snapshots, or the live application; it contains this machine's memory limits
and rendering procedures.

### The verification ladder

Sixty-one percent of Adam's working time goes on `cargo`, and almost all of it
on workspace-wide runs reached for out of caution rather than need. **Stay on
the lowest rung that can see the thing you just changed, and climb only when
you are about to stop.**

| Rung | Command | Cost | Use it |
| --- | --- | --- | --- |
| 1 | `cargo check -p <crate>` | ~1 s | after every edit |
| 2 | `cargo test -p <crate>` | 4 s laptop / 18 s box | after every edit that changes behaviour |
| 3 | `cargo test --workspace --exclude mooloop-ui` | 43 s | before handing work over |
| 4 | `cargo test --workspace` + `cargo clippy --workspace --all-targets` | 118 s + ~88 s | before committing, and nothing smaller than a milestone |

- **`cargo test --workspace` is a commit-time command, not an iteration-time
  command.** One measured session spent 104 minutes on twenty-one of them,
  and sixty-one of its sixty-five test runs passed. The time went on
  confirming nothing had broken, not on finding anything.
- **Never run rung 4 to find out whether something compiles.** That is
  rung 1, and it is a hundred times cheaper.
- **Iterate on the laptop, verify on the box.** Rungs 1 and 2 on a single
  small crate are eightfold faster locally. Rungs 3 and 4, and anything
  touching `mooloop-ui`, need memory the laptop does not have -- send them to
  the remote build box with `scripts/antibox`, which picks incremental
  compilation for dev builds and sccache for release builds on its own.
- **`.slint` edits do not need a build at all.** `scripts/slint-sketch`
  type-checks against the real widgets in about 0.05 s, where a
  `mooloop-ui` build is minutes.

Rung 3 earns its place because `mooloop-ui`'s seven test binaries are most of
what a workspace run compiles and links; excluding them is 70% off.

### Order device work so the face contract comes last

A device has three parts and they differ by four orders of magnitude:

| Part | Cost to see it | Where |
| --- | --- | --- |
| DSP and engine code | 1-4 s | laptop |
| Face markup, visual only | 0.05 s via `scripts/slint-sketch` | laptop |
| The face *contract* — a new property or callback crossing `main.slint` and `lib.rs` | 30 s to check, 8.7 min to a release binary | box |

So get the DSP right against unit tests, iterate the face visually with
`slint-sketch`, and cross into `main.slint` **once**, with every new property
and callback batched into that one pass. The eight-minute build belongs once
per device feature, not once per knob.
