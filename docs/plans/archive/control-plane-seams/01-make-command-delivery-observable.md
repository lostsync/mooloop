# 01 — make command delivery observable

`EngineHandle::send` and `send_structural` discard a full ring, and the five
session reconcilers advance their "last sent" mirrors as though the send
landed. The mirrors are diff-based, so a dropped command is not retried on the
next tick — it is never retried, because the diff it would have been found by
has already been satisfied.

## The state today

`crates/mooloop-engine/src/lib.rs:498`:

```rust
pub fn send(&mut self, cmd: EngineCommand) {
    let _ = self.cmd_tx.push(RealtimeCommand::Engine(cmd));
}
```

`send_structural` and `preview` are the same shape. `install_project`
(`lib.rs:636`) is **not** — it returns `bool`, and `install_project_in_ui`
checks it and leaves both the sample slots and the visible project untouched
on a refusal. That is the pattern this step spreads.

The five reconcilers in `crates/mooloop-session/src/engine.rs` that set a
mirror unconditionally after sending:

| Function | Mirror |
| --- | --- |
| `sync_track_graph:289` | `track_graph_sent:300` |
| `sync_compensation:318` | `compensation_sent:342` |
| `sync_console_sums:382` | `console_sums_sent:398` |
| `sync_solo:410` | `solo_silenced_sent:425` |
| `sync_audio_graph:459` | `audio_graph_sent:467` |

`sync_compensation` is the interesting one: it sends **per channel and per bus**
inside a loop and then assigns the whole plan once at the end. A partial
failure through that loop leaves the mirror claiming a plan that was only
partly delivered, so the fix there is per-entry, not one bool at the bottom.

This contradicts `docs/AUDIO_ARCHITECTURE.md:64`, which already says queue
overflow must be observable to the sender and that silent divergence between
the visible project and the audible engine is not an acceptable steady-state
contract.

## What to do

1. `send`, `send_structural` and `preview` return `bool` (or a small
   `Delivery` enum if `bool` reads badly at the call sites — `bool` is
   probably fine, `install_project` already uses it).
2. Each reconciler advances its mirror **only on success**. Where the send is
   in a loop, track per-entry rather than all-or-nothing, so the next tick
   resends exactly what did not land rather than everything.
3. `#[must_use]` on the three sends. That is what stops the next one being
   written with `let _ =`, and it is most of the durable value of this step.
4. Decide what a refusal should *say*. A dropped structural command that
   retries silently next tick is fine and needs no surface. One that keeps
   failing is not, and the honest minimum is a `log_error!` the first time a
   reconciler is refused, so the failure has a name when somebody eventually
   hits it.

`send_structural` needs one extra thought: on a refusal the `Box`ed node is
dropped back on the GUI thread, which the doc comment already promises. Make
sure the returned `false` does not leave the caller believing it also needs to
reclaim something.

## Acceptance

A `mooloop-session` test that fills the command ring, runs a reconciler, and
asserts the mirror did **not** advance — then drains the ring, runs the pump
again, and asserts the same command is sent. That test is the whole point of
the step: the current behaviour is not "a command is lost", it is "a command is
lost and the system stops believing anything is wrong".

Rung 2 on `mooloop-engine` and `mooloop-session` while iterating, rung 3
before handing over.

## What this is worth beyond the bug

Every reconciler call site that starts handling a delivery result is a call
site that has begun returning an effect instead of performing one. That is
the first move of the README's sixth item, taken for a reason that stands on
its own.
