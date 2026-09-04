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

## The ladder

Four rungs. The rule is **stay on the lowest one that can see the thing you
just changed, and climb only when you are about to stop.**

| Rung | Command | Cost | Use it |
| --- | --- | --- | --- |
| 1 | `cargo check -p <crate>` | ~1 s | after every edit |
| 2 | `cargo test -p <crate>` | ~4 s | after every edit that changes behaviour |
| 3 | `cargo clippy --workspace --exclude mooloop-ui` + `cargo test --workspace --exclude mooloop-ui` | measure it | before handing work over |
| 4 | `cargo test --workspace`, `cargo clippy --workspace --all-targets` | ~5 min + ~2 min | before committing, and at nothing smaller than a milestone |

The costs on rungs 1 and 2 are measured. **Rung 3's are not, and measuring
them is the first task**: `mooloop-ui` is what makes the workspace runs
expensive — seven test binaries linking at once, per
`docs/AGENT_OPERATIONS.md` — so excluding it is the obvious lever and its
value is a guess until someone runs it.

`spikes/slint-units/verification-rungs.sh` on `spike/slint-split-build` does
exactly that measurement; a run was in flight when this was written and did
not land. Run it, put the numbers in the table above, and then decide whether
rung 3 is worth having. If `--exclude mooloop-ui` turns out to save little,
delete the rung rather than keeping a ceremony.

## Write it into `AGENTS.md`

The ladder is worth nothing as a document nobody reads. `AGENTS.md`'s
"Verification and operations" section already says *"Choose the smallest
verification that covers the change"* — which is right, and was not enough,
because it gives no way to tell which one that is. Replace it with the table.

Two rules that need saying explicitly, because their absence is what the
transcript shows:

- **`cargo test --workspace` is a commit-time command, not an
  iteration-time command.** Twenty-one of them in one session is the single
  largest line item in this plan.
- **Never run rung 4 to find out whether something compiles.** That is
  rung 1, and it is three hundred times cheaper.

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
  runner that shares one binary is worth ten minutes of investigation.

## Done when

`AGENTS.md` carries the ladder with measured costs on every rung, rung 3's
value is known rather than assumed, and the mockup tool is behind a feature.

Then re-run the transcript analysis — the script is in this plan's history,
or rewrite it, it is forty lines — against the next long session and see
whether the workspace-run count came down. That number, not a feeling, is
whether this step worked.
