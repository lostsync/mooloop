# 03 — The guard, and breaking it on purpose

`04-guard.md`: the fix is the smaller half. This fault recurs because the
conditions recur — a number that is obviously 4, in a crate that cannot see the
other seven — and the guard is what changes them.

## Four checks

### A. Facts about one value → unit tests beside it

In `mooloop-core/src/time.rs`:

- `STEPS_PER_BAR == STEPS_PER_BEAT as u32 * BEATS_PER_BAR`. It is derived after
  step 01, so this asserts the derivation is still the one intended rather than
  that two numbers match.
- `frames_per_bar(48_000, 120.0)` is `BEATS_PER_BAR` times a beat, not a
  remembered `96_000.0`. `sampler.rs:881` currently asserts the answer; assert
  the relationship as well, or the constant can move and the test stays green.
- `BbtPosition::from_ticks(Ticks(0))` is `1:1:0`. `BbtDuration::from_ticks(one
  bar)` is `1:0:0`. **These two assertions are the whole reason there are two
  types**, so they belong in the same test with a name that says so.
- A sweep over several bars asserting `BbtPosition.beat - 1 ==
  Ticks::beat_in_bar`, which is the invariant step 02 relies on when
  `transport.rs` subtracts one.

### B. A contract between two named things → beside `transport.rs`

`engine::transport::beat_in_bar()` against `BbtPosition::from_ticks` over a
sweep of tick values crossing several bars, including a negative-adjacent case
if `rem_euclid` can ever see one. This is the check that would have caught the
two derivations parting, which is the defect that actually existed.

Count what the sweep covered and assert the count, per `04-guard.md`: a loop
whose range computes to empty passes in silence.

### C. A lead generator over the whole tree → `scripts/dupe-audit`

A new check, `bar-arithmetic`, walking `crates/**/*.rs` — **`read_dir`, never a
list**; the meter-floor guard's first version swept the two files the note
named and would have passed with 26 of 50 copies in place.

It reports lines that mean "four beats to a bar" outside the one file allowed
to say so: `240.0`, `rem_euclid(4)`, `% 4` and `/ 4` on a line also mentioning
`bar`, `beat` or `tick`, and `BEATS_PER_BAR` declared anywhere but `time.rs`.

Exclusions by name with the reason, per `04-guard.md`:

- `crates/mooloop-core/src/time.rs` — the home. Excluded by name, so "this file
  may say four" stays a claim about one file rather than a category others can
  join.

The docstring says what it deliberately misses, because every check in that
script does: a renamed local, `f64::from(BEATS_PER_BAR)` spelled as
`f64::from(4)`, and any bar arithmetic that reaches four by multiplying two
other numbers. `01-find.md` is blunt about this — a clean `repeated-line` is
weaker evidence than it looks, and the copy most likely to diverge is the one
somebody edited on the way past.

### D. The boundary contract → `crates/mooloop-ui/tests/roll_metrics.rs`

That binary already owns "the markup's tick count is the engine's". This is the
same subject one layer up: **the markup's format is the engine's.**

For this to be testable, `BbtText` computes into an `out property <string>` and
its `Text` draws that property — a harness can lift a property into the
generated Rust API, and cannot read a rendered glyph. Then: build the harness
with a known bar/beat/tick, and assert the markup's string equals
`BbtPosition { .. }.to_string()`.

Reuse the binary rather than adding one; each test binary is a separate link of
a large crate.

## Mutation, every time

`05-verify.md`: until a check has failed on purpose, it is a claim with no
evidence. **One mutation at a time** — cargo stops at the first failing binary,
so a batch leaves checks that never ran.

| Mutation | The check that must fail, and what it must say |
|---|---|
| **The tree as it is before step 01** | `dupe-audit bar-arithmetic` reports all eight sites from `00-status.md`. This is the strongest one: the check finds the thing it was written for, in the state it was actually found in. Run it *before* the fix or it is decoration. |
| `STEPS_PER_BAR` back to a literal `16` | A, naming the derivation, not just "16 != 16" |
| `frames_per_bar` back to `240.0` | A, and `dupe-audit` reports `sampler.rs` again |
| `buffer_device.rs` back to its own `240.0` | `dupe-audit` reports it |
| `transport.rs` `beat_in_bar` off by one | B, naming the tick where the two derivations parted |
| B's sweep range emptied | B's count tripwire, not a silent pass |
| `BbtDuration::from_ticks` made 1-based | A: a one-bar duration reads `2:1:0` |
| `BbtPosition::from_ticks` made 0-based | A: tick zero reads `0:0:0`, and `session/engine.rs:755` fails too |
| `BbtText`'s separator changed to `.` | D, markup against `Display` |
| `BbtText`'s `out property` removed | D fails to build — acceptable, and say so in the test's comment so the next person knows the build break is the check working |

## The ladder

`mooloop-core` and two engine crates change, so:

- `cargo check -p <crate> --all-targets` while iterating — **`--all-targets`,
  not the bare form**; moving a symbol out of an import list passes the bare
  check and fails minutes later on a test helper.
- `scripts/slint-sketch crates/mooloop-ui/ui/main.slint` for step 02's markup,
  about two seconds.
- `cargo test --workspace`, redirected not piped.
- `cargo clippy --workspace --all-targets -- -D warnings`. A bare `cargo
  clippy` exits 0 on things CI rejects; `-D warnings` is the whole difference.
- Read the exit code from the command itself, and `grep` the log for each new
  test's name. `0 passed; 0 filtered out` is not a pass.

## Done when

- All four checks exist, and every row of the mutation table has been run and
  its message read.
- `dupe-audit bar-arithmetic` is clean on the fixed tree and was not clean on
  the tree before it.
- `CONTRIBUTORS.md` bumped.
- `CURRENT.md` **not** updated, because nothing the application does has
  changed. If it needs updating, go back and find out what moved.
