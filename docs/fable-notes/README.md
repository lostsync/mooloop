# Fable notes

Lessons from the overnight whole-system review runs (`reports/fable-*.md`).
One file, updated in place rather than duplicated. Each entry says what was
learned and the date it was last confirmed true. Delete an entry when the tree
makes it false.

## Before reporting anything, read what is already known

Confirmed 2026-09-16. This repository already records most of what a first
read of the code turns up, and a finding that is already written down is not
a finding. Read, in this order, before drafting a report:

- `docs/LOOSE_ENDS.md` (about 1,200 lines) — the catalogue of known, deliberate
  stopping points, each with a file and line.
- `docs/SCOPE.md` §5 — the four verified blockers between the code and CLAP
  hosting, the absence of any audio input path, and the fact that
  `driver.rs` is deliberately not a trait.
- `docs/FOCUS.md` "Fixes that may interrupt the sequence" and "Deliberately
  not now".
- `docs/plans/archive/control-plane-seams/README.md` and `00-status.md` — a
  prior outside review of the same seams, with what it found and fixed.
- `docs/AUDIO_ARCHITECTURE.md` "Realtime Executor" — the audio-thread contract
  the code is held to.

The useful report is the delta: what those documents claim that the source no
longer supports, and what the source does that no document has noticed.

## The remote container's clone is shallow; unshallow it first

Confirmed 2026-09-16. The container clones with a grafted root at
2026-09-13 (`30ba940`, 114 commits visible), so `git blame` and `git log -S`
reach three days and everything older blames to that root. It is not a
squash: `git fetch --unshallow origin` works through the proxy, takes under a
minute, and yields the full 811 commits back to 2026-08-11. Do it before any
history question, and check `git rev-parse --is-shallow-repository` before
believing a "this all landed in one commit" story.

Two things about the history once it is there. About 260 of the 811 commits
carry no `Co-Authored-By` trailer; those are the non-Claude harnesses in
`CONTRIBUTORS.md` (Codex, opencode, Kimi, Zed), so a trailer census
under-counts them and the journal is the better witness for who built what.
And `docs/JOURNAL.md`'s early short hashes (`b8a3fd1`, `7e58b4b`, …) resolve
only after the unshallow.

## Building and testing in the remote container

Confirmed 2026-09-16.

- `.cargo/config.toml` pins `-fuse-ld=mold`, which is not installed. Set
  `RUSTFLAGS=""` the way `.github/workflows/ci.yml` does, or every build
  script fails at link time with "cannot find 'ld'".
- `jack-sys` needs `libjack-jackd2-dev` (and `pkg-config`). `apt-get` works
  through the proxy. Without it `cargo test -p mooloop-engine` dies in a
  build script before compiling anything of ours.
- `cargo fetch` and `cargo test -p mooloop-engine --no-run` both work after
  that; rung 2 of the `AGENTS.md` ladder is the right level for this run.
- The CodeGraph MCP server `AGENTS.md` asks for is a local service and is
  unreachable from the container ("ConnectionRefused"). Use `Grep` and
  reading; do not report its absence as a finding.
- Do not run Cargo commands concurrently (`AGENTS.md`). Background one run
  and do something else.

## Commit the report skeleton before the first finding

Confirmed 2026-09-17. `reports/` did not exist in the tree at the start of
this run: the 2026-09-16 report these notes describe was written but never
committed, and the container that held it is gone. Commit an empty
`reports/fable-<date>.md` in the first ten minutes and push after every
finding. It is Markdown, so it goes on `main` directly.

Two smaller container facts from the same run. `apt-get install` fails with
a 404 on a stale index; run `apt-get update` first. And `libasound2-dev` is
not needed — `libjack-jackd2-dev` alone gets `cargo test -p mooloop-engine`
building, and the suite passes in about twelve seconds once built.

## Read the last two days of `main` before reading the tree

Confirmed 2026-09-17. The one HIGH finding of this run — the Reverb face
reading `p0..p7` after `EffectSlotRow` switched to id-indexed fill — was
introduced the previous day by a commit that changed a shared scheme and
audited it against the one face it was working on. `git log --since='2 days'
--stat` and a look at every file a scheme change *did not* touch is faster
than any grep, and it is where a whole-system review can see what a
single-task session cannot.

CI green is not evidence for a UI row: `source_snapshot.rs` builds its
`EffectSlotRow`s by hand, so nothing in CI runs `effect_slot_row` for most
kinds. Check what a test constructs before believing it covers a path.

## Working method that held up

Confirmed 2026-09-17.

- Three parallel investigators (seams, real-time safety, architecture fit)
  each briefed with the already-known list, then a fresh-context refuter per
  finding set. The refuter is where the report earns its keep: brief it with
  the exact file:line claims and ask it to prove them wrong.
- Severity is about what a user hears or loses, not about how wrong the code
  looks. A contract violation that cannot be reached is a Low.
