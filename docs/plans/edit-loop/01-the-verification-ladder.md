# 01 — The verification ladder

Read `00-status.md` first. This step changes no application code and is
expected to be most of the win.

## The finding it acts on

61% of Adam's working time goes on `cargo`, and in the measured session
166 of 172 cargo-minutes were 48 workspace-wide runs — 21 of them
`cargo test --workspace` at five minutes each, 61 of 65 of which passed.
Meanwhile the targeted inner loop, 47 runs of `-p mooloop-session`, cost two
and a half minutes in total.

Nobody decided to do that. It happened because there was no rule about which
command to reach for, so the safest one got reached for every time.

## Do this first: turn incremental compilation back on

`scripts/antibox` sets `CARGO_INCREMENTAL=0` whenever sccache is wrapping
rustc, with the comment *"rustc refuses to hand sccache an incremental
compilation unit, so an incremental build would silently bypass the cache
entirely."* That is true, and it is the right trade for the first build of a
fresh checkout. It is the wrong one for an edit loop, where incremental
compilation is the entire point -- and the default box path is the one every
verification run goes through.

Measured on the box, one-line edit in `mooloop-session`:

| Command | sccache (incremental off) | `--no-sccache` (incremental on) |
| --- | --- | --- |
| `cargo test -p mooloop-session` | 31 s | **18 s** |
| `cargo test --workspace --exclude mooloop-ui` | 141 s | **43 s** |
| `cargo test --workspace` | 332 s | **118 s** |
| `cargo build --release -p mooloop-app`, `main.slint` edit | **522 s** | 672 s |

**64% off the workspace test run, from a flag.** The last row is why it has
to be a choice rather than a new default: release builds are 29% *worse*
with incremental, gaining little from it and losing sccache's dependency
cache.

So the rule is by profile, not by preference:

- **dev-profile `check`, `test`, `clippy` -- incremental on**, sccache off;
- **release builds -- sccache on**, incremental off.

Put it in `scripts/antibox` rather than leaving it to whoever remembers the
flag. Picking the mode from whether `--release` appears in the command is
probably the cleanest form, with an override for both and `--no-sccache`
still meaning what it means now.

**This is the cheapest thing in this plan.** On the measured session it takes
21 workspace test runs from 104 minutes to 41, and it needs no discipline
from anybody.

## The ladder

Four rungs. The rule is **stay on the lowest one that can see the thing you
just changed, and climb only when you are about to stop.** Costs are the box
with incremental on, after a one-line edit; rungs 1 and 2 are quoted from the
laptop, where they are faster still.

| Rung | Command | Cost | Use it |
| --- | --- | --- | --- |
| 1 | `cargo check -p <crate>` | **~1 s** laptop | after every edit |
| 2 | `cargo test -p <crate>` | **4 s** laptop / 18 s box | after every edit that changes behaviour |
| 3 | `cargo test --workspace --exclude mooloop-ui` | **43 s** | before handing work over |
| 4 | `cargo test --workspace` + `cargo clippy --workspace --all-targets` | **118 s** + ~88 s | before committing, and nothing smaller than a milestone |

Rung 3 earns its place: excluding `mooloop-ui` is 70% off, because its seven
test binaries are most of what a workspace run compiles and links. With
nothing changed at all the whole workspace's tests *execute* in 46 s, so
nearly everything above that is build cost rather than test cost.

Rungs 1 and 2 belong on the laptop, which beats the box eightfold on a single
small crate because it has an incremental cache and no rsync. Rungs 3 and 4
belong on the box, which has the memory.

## What the flag and the rule are worth together

On the measured session -- 21 runs at rung 4, 18 clippy runs, and nothing in
between -- with most of that traffic moved to rung 3 and the flag fixed:

| | Today | With this step |
| --- | --- | --- |
| workspace test runs | 21 runs, 104 min | 18 at rung 3 + 3 at rung 4, **19 min** |

Eighty-five minutes off a 224-minute session, from a flag and a rule.

## Write it into `AGENTS.md`

The ladder is worth nothing as a document nobody reads. `AGENTS.md`'s
"Verification and operations" section already says *"Choose the smallest
verification that covers the change"* — which is right, and was not enough,
because it gives no way to tell which one that is. Replace it with the table.

Three rules that need saying explicitly, because their absence is what the
transcript shows:

- **`cargo test --workspace` is a commit-time command, not an
  iteration-time command.** Twenty-one of them in one session is the single
  largest line item in this plan.
- **Never run rung 4 to find out whether something compiles.** That is
  rung 1, and it is a hundred times cheaper.
- **Iterate on the laptop, verify on the box.** A single small crate is
  faster locally; the workspace and anything touching `mooloop-ui` is not
  survivable locally.

## Two smaller wins while in here

- **Take the developer tooling out of the shipping build.**
  `crates/mooloop-ui/ui/main.slint:50` and `:53` re-export `MockupCanvas` and
  `MockupCatalog` so `mockup.rs` can construct them, which pulls the mockup
  tool into the same compilation unit as the window: 1.78 MB of generated
  Rust, 4.3% of the module, in every build including release. Put it behind
  a Cargo feature.
- **Check whether `cargo-nextest` helps.** It is not installed and may not
  be worth a dependency, but `cargo test --workspace` at five minutes is
  dominated by building and linking seven `mooloop-ui` test binaries, and a
  runner that shares one binary is worth ten minutes of investigation. Rung
  3 already gets 70% of this by excluding them; nextest would have to beat
  that to be worth a dependency.

## Done when

`scripts/antibox` picks incremental or sccache by profile instead of always
sccache, `AGENTS.md` carries the ladder with its measured costs, and the
mockup tool is behind a feature.

Then run `scripts/loop-profile` against the next long session. The number to
watch is the share of active time spent waiting on cargo -- 61% across the
nineteen sessions this plan was written from -- and the count of rung-4 runs
in a single session, which was twenty-one. Those two numbers, not a feeling,
are whether this step worked.
