# Stop rewriting Slint models with identical data every 8ms

## Problem

The pump timer (`PUMP_INTERVAL_MS = 8`, `crates/mooloop-ui/src/lib.rs:48`)
runs at 125Hz and unconditionally touches several Slint models regardless
of whether their content actually changed:

1. `state.playhead_model.set_vec(positions)`
   (`crates/mooloop-ui/src/lib.rs:5494`) — `VecModel::set_vec` fires a
   full model **Reset** notification (tears down and rebuilds repeated
   items in Slint), every tick, even when `positions` is empty and was
   already empty on the previous tick (the common case: no sampler
   selected, or a non-playing sampler).
2. `state.effect_slot_model.set_row_data(slot, row)` in the meter-publish
   loop (`crates/mooloop-ui/src/lib.rs:5462-5476`) — `set_row_data`
   unconditionally notifies Slint's dependency tracker, so a slot with a
   silent (0.0 dB / already-zero) meter still marks that row dirty and
   forces downstream bindings (any UI element reading that row) to
   re-evaluate, every tick, whether or not the rack is even visible.
3. `mixer_strip_model.set_row_data(bus, row)` similarly, gated only by
   `showing_mixer` (`lib.rs:5432`) — good, already conditional — but not
   by whether `left_db`/`right_db` actually moved since last tick.

## What to do

1. **`playhead_model`**: only call `set_vec` when `positions` differs
   from what's currently in the model, or more simply, track whether the
   previous tick's positions were empty and skip the call if this tick's
   are also empty (`Vec::is_empty()` check both sides — avoids the
   allocation in `handle.playhead_positions()` too, if that call itself
   is skippable when nothing is playing on a non-sampler-selected
   channel; check whether it's cheap enough to just always call and only
   skip `set_vec` on the empty-to-empty case).
2. **`effect_slot_model`** meter writes: skip the `set_row_data` call
   when the newly computed `in_l/in_r/out_l/out_r` (in dB, rounded to
   whatever display precision the UI actually shows) equal the row's
   current values. A cheap epsilon compare against the last-written value
   is enough — don't reintroduce per-frame allocation to do this (e.g. a
   small `Vec<(f32,f32,f32,f32)>` cache sized to `MAX_EFFECTS_PER_CHANNEL`
   per selected chain, reused across ticks, is fine).
3. Also gate the whole effect-slot meter block on rack visibility if such
   a flag exists (check whether `main.slint` exposes something like
   `rack_visible`/`device_rack_visible` analogous to `mixer_visible`) —
   if the rack can be hidden while a project plays, there's no reason to
   touch these models at all while it's off-screen.
4. Apply the same "skip if unchanged" treatment to `mixer_strip_model`'s
   `left_db`/`right_db` writes even though it's already gated by
   `showing_mixer`.

## Verification

- `cargo test -p mooloop-ui` — snapshot tests
  (`crates/mooloop-ui/tests/mixer_snapshot.rs`,
  `crates/mooloop-ui/tests/rack_tools.rs`,
  `crates/mooloop-ui/tests/source_snapshot.rs`) must still pass; they
  presumably drive the pump synchronously or check model contents
  directly, so a "skip if unchanged" path must never mean "skip the
  *first* write."
- Manual: play a project with a sampler channel selected so playhead
  markers move, confirm they still animate smoothly; select a
  non-sampler channel and confirm no console warnings/panics from an
  empty-to-empty skip path; open the mixer and rack simultaneously and
  confirm meters still respond to actual level changes, not just frozen
  after the first non-silent tick.
- If `SLINT_DEBUG_PERFORMANCE=refresh_lazy,console` is set (see
  `docs/plans/reduce-ui-pump-overhead/03-verify-with-slint-perf-logging.md`),
  confirm layer/redraw counts drop for an idle-but-playing project
  (silence, or a muted channel) compared to before this change.
