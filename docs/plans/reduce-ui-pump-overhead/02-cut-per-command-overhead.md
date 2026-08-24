# Remove per-command allocation and syscalls from the pump's forward loop

## Problem

The pump's command-forwarding loop
(`crates/mooloop-ui/src/lib.rs:5197-5220`, inside
`while let Ok(message) = pending_rx.try_recv()`) does several things
per-message that should be per-tick or per-change instead:

1. `std::env::var("MOOLOOP_AUTODRIVE_VERBOSE").is_ok()`
   (`lib.rs:~5209`) is called **inside the per-command match arm** — every
   knob turn, every mouse-move during a drag that generates a
   `SetEffectParam` command, takes an env lookup (which locks and
   allocates on most platforms) on the UI thread. This is diagnostic-only
   code that should cost nothing when unused.
2. `update_document_title` (`lib.rs:1353`) — called from inside the
   forward loop whenever a non-transport command is seen — always does a
   `format!()` allocation and a `window.set_document_title(...)` call,
   even when `state.dirty` was already `true` from a previous command
   this same tick (i.e. the title string wouldn't change). During a
   continuous knob drag this fires once per forwarded command, not once
   per tick.

## What to do

1. Hoist the `MOOLOOP_AUTODRIVE_VERBOSE` check out of the loop: read it
   once via `std::sync::OnceLock<bool>` (or read it once at pump-setup
   time, outside the `Timer::start` closure, and capture the `bool` by
   move) so the per-command cost is a single `bool` copy, not an env
   lookup.
2. In the `dirty`-marking branches inside the forward loop
   (`lib.rs:5197-5220`), only call `update_document_title` when
   `state.dirty` actually transitions `false -> true` this tick, not on
   every command while it's already `true`. Track a local
   `title_needs_refresh: bool` for the duration of one pump tick,
   accumulate it across every message in the drain, and call
   `update_document_title` at most once at the end of the
   `pending_rx` drain loop instead of inside it — this also naturally
   collapses N commands in one tick to 1 title update instead of N.
3. Check whether the same "only touch it once per tick, not once per
   message" pattern applies to `sync_command_availability` calls inside
   the same loop (`lib.rs:5237` and others) — if it's cheap (just enables
   flags), leave it; if it walks a model, apply the same tick-level
   coalescing.

## Verification

- `cargo test -p mooloop-ui` — no test should depend on the title being
  updated mid-drain (check `preferences_audio_snapshot.rs` and any test
  touching `document_title`); if one does, adjust it to check the
  end-of-tick state rather than an intermediate one.
- Manual: drag a knob continuously for a couple of seconds, confirm the
  document title still shows the `*` (dirty) marker promptly (within one
  pump tick, i.e. ~8ms, not delayed further) and doesn't flicker or lag
  behind.
- Confirm `MOOLOOP_AUTODRIVE_VERBOSE=1` still prints per-command traces
  correctly after the hoist (the autodrive self-test at
  `lib.rs:~5509` exercises dozens of commands in one burst — good manual
  check).
