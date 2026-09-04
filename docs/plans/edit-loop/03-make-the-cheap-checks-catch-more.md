# 03 — Make the cheap checks catch what the expensive ones are catching now

Read `01-measure-the-loop.md` first. This step only makes sense if its
question 1 came back as "test failures" or "behaves wrong" -- if the answer
was "compile errors", that step already has the fix and it is free.

## The idea

There are five verification layers and they differ by three orders of
magnitude:

| Layer | Cost | Catches |
| --- | --- | --- |
| `scripts/slint-sketch` | 0.05 s | markup that does not type-check, and how a widget looks |
| `cargo test -p mooloop-session` | under 1 s, 87 tests | every edit, undo, and engine command the model makes |
| `cargo check` on a small crate | seconds | Rust that does not compile |
| `cargo test -p mooloop-ui` | minutes | the window, its snapshots, its interaction |
| build and run it | minutes plus a human | everything else |

Every failure that reaches the bottom row and could have been caught in the
top two is pure waste, and it is the waste that makes a cycle 45 minutes.
The work here is moving failure detection upward.

## Three concrete moves

**Push logic down into `mooloop-session`.** It has no `slint` dependency, its
87 tests run in under a second, and it already owns the model, the edits,
undo, and engine command emission. Anything currently decided in
`mooloop-ui/src/lib.rs` that is not about drawing belongs there instead --
the same reasoning `docs/plans/session-layer-extraction/` used, applied to
whatever the measured cycle in step 01 shows going wrong. Start from the
actual failures, not from a survey.

**Take the developer tooling out of the shipping build.**
`crates/mooloop-ui/ui/main.slint:50` and `:53` re-export `MockupCanvas` and
`MockupCatalog` so `mockup.rs` can construct them, which pulls the mockup
tool into the same compilation unit as the window: measured at 1.78 MB of
generated Rust, 4.3% of the module, in every build including release. Put it
behind a Cargo feature. It is a small win on its own and it is the cleanest
possible demonstration that the module's size is a thing anyone can move.

**Make the headless harness reachable for the thing that broke.** If step
01's cycle failed on behaviour, ask what test would have caught it and
whether that test could have run in a second rather than a minute.
`crates/mooloop-ui/tests` drives the real window in-process through
`i-slint-backend-testing`; `mooloop-session`'s tests need no window at all.
The question to answer for each real failure is which of those two it should
have been.

## What not to do

Do not write a test suite. This step is not "improve coverage" -- it is
"stop the same failure from costing a four-minute build twice". Take the
failures step 01 actually recorded, one at a time, and move each one up a
layer. If there were only two failures and both were behavioural and
unavoidable, this step is finished and should say so.

## Done when

Each failure recorded in step 01 has either been moved to a cheaper layer or
been written down as genuinely needing the expensive one, and the mockup tool
no longer compiles into release builds.
