# 02 — Get the build off the laptop

Read `01-measure-the-loop.md` first. Do not start this until its question 2
is answered; if the builds are already happening on the box, skip to `03`.

## Why this is first

The laptop has 15 GB and, during the session that wrote this plan, had zero
free and 8 GB of zram swap fully consumed with an ordinary desktop running.
`cargo check -p mooloop-ui` needs 3.2–3.4 GB in a single rustc process and
was killed by `scripts/cargo-capped`'s cgroup three times in a row. The box
has 62 GB and eight cores and does the same check in 69 s.

`cargo build -p mooloop-ui` is about four minutes on the laptop. On the box
a full `mooloop-app` rebuild after a `main.slint` edit is 56 s. If a cycle
involves three of those, that is nine minutes of the forty-five, and it goes
to under three.

## What exists already

`scripts/antibox --release-bin` compiles `mooloop` with `--release` on the
box, strips it, and copies it to `./bin/mooloop-test`.
`docs/AGENT_OPERATIONS.md` documents it. It may already be the answer, in
which case this step is confirming that and making it the default rather
than a thing to remember.

## What is probably missing

- **A dev-profile equivalent.** `--release-bin` only builds release. The
  workspace's dev profile is deliberately tuned to be playable --
  `opt-level = 1` workspace-wide, with a comment in `Cargo.toml` saying the
  ordinary `cargo run` path is the one used to play the instrument -- so a
  dev build pulled from the box may be both usable and much faster. Measure
  whether it is: build both, run both, and find out whether the dev binary
  actually holds up under JACK with a real song, or whether the release
  build is genuinely required.
- **A one-command loop.** Whatever the answer, the shape wanted is a single
  command that edits are followed by: build on the box, copy the binary
  down, run it locally against JACK. Not three commands and a path to
  remember. `scripts/antibox --release-bin && ./bin/mooloop-test` is nearly
  it already; if the dev path works, add `--dev-bin` beside it.
- **Debug symbols.** `--release-bin` strips the binary, so a crash gives no
  backtrace. That is the right default for listening and the wrong one for
  diagnosing. If a cycle is ever spent on "it crashed and I do not know
  where", give the script a way to keep them.

## The obvious trap

Do not make the box mandatory. It is a second machine that can be full,
unreachable, or busy -- during the session that wrote this plan it was 100%
full at 436 GB and blocked every remote command until 170 GB of dead
per-branch target directories were pruned. Whatever this step produces has
to degrade to a local build rather than fail.

**And fix the thing that caused that**, since it will happen again:
`scripts/antibox` keys its remote target directories by absolute checkout
path and never removes them, so every branch ever built leaves 10–20 GB
behind forever. Teach it to prune directories whose checkout path no longer
exists, or to prune by age. This is twenty lines and it protects the whole
plan.

## Done when

There is one command that turns an edit into a running application, it does
not use the laptop's memory to compile, it falls back gracefully when the
box is unavailable, and the box no longer fills itself up.
