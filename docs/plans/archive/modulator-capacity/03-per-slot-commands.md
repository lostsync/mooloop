# 03 — Per-slot commands

`SetChannelModulation` ships an entire `ModRack` by value. That is why the
command ring is sized by capacity: 936 bytes an entry at eight slots,
2760 at thirty-two, times a preallocated 1024 entries.

`bridge.rs` defends the width, and it is right to: a wide variant costs
fixed setup memory rather than a per-command allocation, and boxing would
put a drop on the realtime callback. The defence is about *boxing*, not
about *granularity*. Turning one LFO knob currently ships the whole rack —
every module, every route — when the fact that changed is one `f32`.

## The shape

- Add narrow commands beside the wide one: set one slot's params, install
  or clear one slot, add or retune one route, remove one route. The ring
  entry is then sized by the widest single module (~76 bytes) rather than
  by the rack, and stops growing with capacity entirely.
- Keep `SetChannelModulation` for the cases that genuinely replace the
  whole rack: project load, channel preset, undo of a structural edit.
  Those are rare and already wide.
- The engine's existing whole-rack diff is what restores a destination's
  base when a route disappears. Per-slot commands need that same restore
  incrementally, which is the real work of this step and the reason it is
  sequenced last: it is the only part that can get modulation *wrong*
  rather than merely expensive.

## The trap to avoid

Ordering. A slot edit and a route edit that arrive in separate ring
entries must not leave the engine resolving a route against a module that
has not landed yet, or against one that just left. The rule to hold is the
one the rack already follows off the realtime thread: a route names a
durable `ModSourceId`, so a route command that names an absent source is
inert rather than misaimed, exactly as `UNRESOLVED_SLOT` already makes it.
That property is why this step is cheap now and would not have been before
step 03 of the modulator-modules plan.

## Explicitly not in this step

- No change to how the rack is persisted or addressed. This is a
  transport change; `ParamAddr`, routes, and the descriptor contract are
  untouched.
- No shrinking of `QUEUE_CAPACITY`. Fewer bytes per entry is the win;
  keeping the same generous depth is the point.

## Done when

- An ordinary knob turn on a modulator ships a small command, verified by
  the ring-cost test dropping to the single-module width.
- A route removed by a narrow command restores its destination's base at
  the next block, with the same test that covers the wide path today.
- Capacity changes stop moving the ring line in `00-status.md`'s table.
