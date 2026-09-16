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

## Git history starts on 2026-09-13

Confirmed 2026-09-16. The root commit (`30ba940`, 841 files, 1.89 M lines) is
a squash dated 2026-09-13, and every commit since carries a Claude Opus 5
co-author trailer bar one Sonnet commit. `CONTRIBUTORS.md` lists ten
model+harness pairs working from 2026-08-21, so **the multi-agent divergence
the review is asked to trace is almost entirely before the reachable
history.** `git blame` and `git log -S` reach three days.

What survives from before the squash is `docs/JOURNAL.md`: a reconstructed
narrative with dated sections from Aug 11 and the original short hashes
(`b8a3fd1`, `7e58b4b`, …), which no longer resolve in this clone. Use it as
the archaeology; cite it as prose, not as commits.

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

## Working method that held up

Confirmed 2026-09-16.

- Three parallel investigators (seams, real-time safety, architecture fit)
  each briefed with the already-known list, then a fresh-context refuter per
  finding set. The refuter is where the report earns its keep: brief it with
  the exact file:line claims and ask it to prove them wrong.
- Severity is about what a user hears or loses, not about how wrong the code
  looks. A contract violation that cannot be reached is a Low.
