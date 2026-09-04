# Investigate and fix the apparent 8-effect ceiling

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "Adding more than 8 effects seems to
still be disabled. I thought this restriction had been dealt with. An
internal limit is fine but it should be high enough that your cpu would
choke from DSP before you hit it. Might just be UI lagging behind
internals."

The backend is not the ceiling: `MAX_EFFECTS_PER_CHANNEL` is
`u8::MAX as usize + 1` = 256 (`crates/mooloop-core/src/channel.rs:18`),
and the only length check on the UI side, `effects.len() >=
MAX_EFFECTS_PER_CHANNEL` (`crates/mooloop-ui/src/lib.rs:4492`), gates at
256 too. Nothing in `mooloop-engine`'s `EffectChain` (fixed-size arrays,
`crates/mooloop-engine/src/render.rs`) special-cases 8 either. So the
block is somewhere else — most likely a rack-width/scroll assumption in
`device-rack.slint`, a stale disabled-state binding on the insert control,
or an 8-entry model that was never resized when the backend limit was
raised.

## What to do

1. Reproduce: add effects one at a time in the running app past 8 and
   observe exactly what happens — does the "+" control disable, does the
   rack stop scrolling, does the new device silently fail to render, or
   does insertion succeed but something else visually breaks?
2. Trace the insert path from the UI control back: the rack's insert
   trigger (`DeviceFrame`'s "+" in `device-rack.slint`) → whatever Rust
   callback handler in `mooloop-ui/src/lib.rs` mutates the effect list →
   the model backing the rack's `ListView`/repeater. Check for any
   literal `8` or a model/array sized before the 256 bump landed.
3. Fix at the actual point of staleness — likely a model capacity or a
   disabled-state condition that references an old constant rather than
   `MAX_EFFECTS_PER_CHANNEL`. Do not raise a separate cap; there should be
   exactly one source of truth.
4. Confirm added effects past 8 actually run audio (not just render in
   the rack) by checking they show live metering.

## Verification

`cargo test -p mooloop-ui` for the model/state test if one exists, plus a
manual/live check: run the app, add 12+ effects to one channel, confirm
each is insertable, orderable, and passing signal.
