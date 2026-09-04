# 01 — Measure the loop as it is actually run

Read `00-status.md` first.

Every number in this plan so far is a number an agent measured on its own
terms. None of them is the number Adam feels, because nobody has watched a
real cycle end to end. Fix that before changing anything.

## Ask Adam these four things

They are not rhetorical and the rest of the plan branches on them.

1. **When it fails, what fails?** A compile error, a test failure, or the
   thing builds fine and behaves wrong? These have completely different
   fixes, and only the third is hard.
2. **What is being run?** `cargo run --release -p mooloop-app` on the laptop,
   `cargo run` without `--release`, or a binary pulled from the box with
   `scripts/antibox --release-bin`?
3. **What is being edited?** Device faces, `main.slint`, `controls.slint`,
   Rust in `mooloop-session` or `mooloop-dsp`? Step 04's whole value depends
   on this and it is currently a guess.
4. **What counts as "tested"?** Listening to it, clicking through it, or
   running `cargo test`?

If the answer to 1 is "compile errors", stop and read the last section of
this document, because that is an agent-behaviour problem and no amount of
build tuning fixes it.

## Then measure a real cycle

Not a synthetic one. Take an actual small change Adam wants -- ideally one
already on the list in `docs/FOCUS.md` -- and time every phase of getting it
in:

- how long the edits take to write,
- how long the first verification takes,
- what it costs each time it comes back,
- how many times it comes back,
- how long the successful run takes.

Write it down as a table in `00-status.md`. One honest cycle is worth more
than any amount of `cargo check` timing, because it is the only thing that
shows where the 45 minutes actually goes.

## The measurement that is probably already interesting

A run is in flight from the session that wrote this plan and may need
repeating: **cold and warm release builds of `mooloop-app` on the box**, plus
what a Rust-only edit costs versus a `.slint` edit. Re-run it with
`scripts/antibox bash spikes/slint-units/release-loop.sh` from
`spike/slint-split-build`, or rewrite it -- it is eight lines.

The specific thing to find out: **is the binary being built on the laptop?**
If it is, that alone is the whole problem. `cargo build -p mooloop-ui` is
about four minutes on the laptop against 56 s on the box, and the laptop
could not even complete a `mooloop-ui` check during the session that wrote
this, because it had zero free memory and a full swap.

## Done when

`00-status.md` has a real cycle written out phase by phase, the four
questions are answered, and the next step is chosen from that rather than
from this document's guesses. It is entirely possible that step 02 turns out
to be the whole fix and steps 03 and 04 are never needed.

## If the failures are compile errors

Then this is not a build-system problem, it is a working-agreement problem,
and the fix is free.

An agent making three sizeable edits and handing them over unverified is
flying blind for three edits, and the failures Adam is absorbing are ones it
could have absorbed itself. The rule that fixes it: **check every edit on the
box before handing anything over.** `scripts/antibox cargo check -p <crate>`
costs 69 s for the worst crate in the workspace and nothing at all for the
rest, and the agent is waiting anyway.

Write that into `AGENTS.md` under "Verification and operations" if it is not
already being honoured, because it costs Adam nothing and it removes a whole
category of the failures he described.
