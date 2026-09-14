# 05 — Verify

A new check is a claim that something would be caught. Until it has failed on
purpose, it is a claim with no evidence behind it.

## Mutation, every time

**Break the thing the check is for, run it, read the message.** Not "does it
compile" — *does it fail, and does it fail saying the right thing*.

From the 2026-09-13/14 runs, every new assertion, and what it said:

| Mutation | What the check said |
| --- | --- |
| `min-db: -60.0` → `-61.0` | `min-db: slint -61 vs rust -60` |
| one `GainMath.min-db` → `-60` | named the file and line that spelled the floor |
| `meter-segments: 14` → `15` | mixer.slint against `MIXER_STRIP_METER_SEGMENTS` |
| aux-in default `0.3552344` → `0.35` | the face rests at 0.35, the table at 0.3552344 |
| `LfoParam.phase: 3` → `4` | the shelf sends 4, mooloop-core says 3 |
| Smoothing `maximum: 2` → `3` | face max 3, table 2 |
| either rate curve → `Linear` | *the knob is drawn `ValueScale.logarithmic`, and the table says Linear* |
| `reset()` forgets only the level | `a reset meter is still latched` |
| plan stops at the chain's end | `[(0,false),(1,true)]` vs `[(0,false),(1,true),(2,false)]` |
| a rail loses `show-clip: false` | `device-rack.slint:250: ... draws a clip lamp nothing can light` |
| the fader row loses `clip-reset` | `... can light and cannot be cleared, which is worse` |
| the knob entry loses its Escape handler | `controls.slint:957`, the line the row pointed at |
| `NameField` loses its Escape handler | `Escape left the caret in the field, so Space would still type a space` |

**Mutate one thing at a time.** Three mutations in one run killed two checks and
the third never ran, because cargo stops at the first failing binary. If you
batch them, expect to re-run for the ones that did not get a turn. (`--no-fail-fast`
is a *cargo* flag; after `--` it goes to libtest, which rejects it.)

**The best mutation is the tree before the fix.** Reverting a curve to `Linear`
reproduced a defect that had been on `main` that morning, which is a stronger
statement than any synthetic break: the check finds the thing it was written
for, in the state it was actually found in.

## The ladder, and the rung that is not what you think

`AGENTS.md` has the ladder. One correction from these runs:

**Rung 1 for a change that alters an import surface is
`cargo check -p <crate> --all-targets`, not `cargo check -p <crate>`.** Moving
`send_edges` out of an import list passed `cargo check -p mooloop-session` and
failed the workspace run minutes later on an engine *test helper*. The
`--all-targets` form finds it in three seconds.

And: iterate `.slint` on `scripts/slint-sketch` (~2 s for the whole window),
send anything touching `mooloop-ui` to `scripts/antibox`, and background
anything above rung 2.

## Three ways a green run lies

**1. The harness's exit code is not cargo's.** A command ending
`; echo "EXIT: $?"` reports the `echo`'s status, so the task notification says
"exited with code 0" for a run that failed. Seen three times in two days: cargo
exiting 101 on a failed test binary, and 255 when ssh dropped as the laptop
slept. **Put `; echo "EXIT: $?"` in the command and read that line**, or read
the `test result:` lines. Never report from the harness's summary.

**2. Green tests are not a green CI.** `cargo test -p mooloop-ui` passed while
`cargo clippy --workspace --all-targets -- -D warnings` failed on a `mut` a
refactor had made unnecessary and a `clone` on a `Copy` type. Run clippy the
way CI runs it or you are not running it — and `-D warnings` is the whole
difference, because a bare `cargo clippy` exits 0 on both.

**3. A test that has stopped existing does not fail.** A `#[test]` attribute
adrift from its function, a walk whose parser no longer matches, a filter that
selects nothing — all report success. Check the test *ran*:

```sh
grep -n "<the test's name>" /tmp/run.log
```

`cargo test -p x --lib foo` printing `0 passed; 0 filtered out` is not a pass.

## Before merging

- `cargo test --workspace` if `mooloop-core` or an engine crate changed;
  `cargo test -p mooloop-ui` if only the UI did.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- Both redirected, not piped, and both exit codes read from the command itself.
- The `LOOSE_ENDS.md` row deleted if the item is gone, **rewritten if the fix
  was broader or narrower than the fault**.
- `CURRENT.md` updated if any of this changed what the application does.
- `CONTRIBUTORS.md` bumped.
