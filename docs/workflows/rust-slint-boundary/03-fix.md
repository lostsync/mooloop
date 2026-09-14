# 03 — Fix

Four shapes. Which one applies is decided by *why* the copy exists, not by how
it looks.

## A. Give the value one home and make both sides read it

The default, and the only one that removes the copy rather than watching it.

`gain.slint` grew `out property <float> min-db: -60.0;`, its two converters
read it, and 50 literals across ten files became references. `mixer.rs` grew
`sends_are_compensable` and both crates' compensation paths call it.

**`out` rather than `in` when the value is a fact rather than a setting.** A
meter may legitimately declare a different scale bottom, but it does so by
overriding its own `minimum-db` — not by moving the floor for everybody.

Two traps:

- **The Rust name and the Slint name may differ for one idea.**
  `ParamCurve::Exponential` and `ValueScale.logarithmic` are the same law read
  from two ends. Write the mapping down where the check lives, or the next
  person finds a disagreement that is not one.
- **A sweep must be sized from the tree, not from the note.** Walk the
  directory. See [01-find.md](01-find.md).

## B. Name the number on the Rust side and check the markup against it

When the markup legitimately needs its own spelling — a layout constant, a
segment count a widget is sized around — the fix is not to share the value but
to stop it being anonymous on one side.

The repaint throttle passed a bare `14` for the mixer strip and `12` for the
device rails, mirroring `MixerMetrics.meter-segments` and the rails' `segments:`.
They became `MIXER_STRIP_METER_SEGMENTS` and `DEVICE_RAIL_METER_SEGMENTS`, and
a test reads the counts back out of the markup.

This is the shape to reach for when A would need a property threaded through
markup that does not otherwise need it.

## C. Decide the policy once, in the crate that owns the concept

For shape 1. Do not "port the guard across" — that makes the two agree today
and does nothing about the next divergence.

The compensation guard moved to `mooloop-core` as a predicate both call sites
read. What did **not** move is the derivation around it: the engine walks
`ProjectChannel.setup` and the session walks its own channel type, identical
arithmetic over different iteration. Sharing that needs a common input type or
a trait, which is a larger change than the drift called for.

**Fix the thing that diverged. Write down the thing that did not.**

## D. Make the inert control honest

For shape 5, where there is no copy — a widget draws from a property nobody
bound. There are two right answers and the fix is to tell them apart:

- **The caller has the data and was dropping it.** A track's fader row had a
  clip latch and a peak hold on the `MeterReading` the pump was already
  reading, published neither, and bound `held-left-db` to the *level*. Wire it.
- **The caller has no such data.** The device rack's rails meter a chain, and a
  chain has no clip latch. Give the component a way to say so —
  `ChannelMeter` grew `show-clip` — rather than inventing a latch to satisfy
  the widget.

## What a fix must not do

**Do not clear-then-restate.** When re-stating a subscription or a
subscription-like flag, state the new value directly. `set_spectrum_enabled`
returns early for an already-correct subscription, while a clear zeroes the
bins — so clearing first would make every open analyzer in the program blink on
an unrelated device drag.

**Do not widen a migration while fixing a default.** `StripBand`'s missing
`position` now comes from a wire type with `position: Option<u8>` filled per
index. `kind`, `gain_db` and `q` stayed non-optional, and there is a test
saying so. A fix to one field's default is not a licence to loosen its
siblings.

**Prefer the option that makes the code match claims already written down.**
`default_band_position` had three options; the one taken was the one where the
doc comment and `PROJECT_FORMAT.md` were already right and only the code was
wrong. The other two would have written the drift down as intent.

**If the fix is broader than the fault, say so in the row rather than
deleting it.** Resetting every non-master meter on any project install fixes an
inherited clip latch and also clears a legitimately lit lamp when somebody
clones a pattern. That trade was chosen deliberately — for an alarm, a false
clear beats a false latch — and the row now carries what the narrow version
would take.

## Cost, so the order is right

`AGENTS.md`'s rule is "order device work so the face contract comes last", and
it applies here:

| Change | To see it |
| --- | --- |
| `.slint` only | `scripts/slint-sketch crates/mooloop-ui/ui/main.slint`, ~2 s |
| Rust only, one crate | `cargo check -p <crate> --all-targets`, seconds |
| A new property or callback crossing `main.slint` and `lib.rs` | one `mooloop-ui` build, ~8 min |

Batch every new property into one crossing. The clip-lamp fix added three
properties and a callback to `main.slint` in a single pass for that reason.
