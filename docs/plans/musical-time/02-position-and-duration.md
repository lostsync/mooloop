# 02 — A position and a duration are different types

Shape A again, for the derivation and the format rather than the constant.

## Two types, because one type with a flag is the bug

A position counts bars and beats from one: tick zero is `1:1:0`, and
`session/engine.rs:755` has a test asserting exactly that. A duration counts
from zero: one bar is `1:0:0`, and getting it wrong prints `2:1:0` for a
one-bar length.

One type with an `is_duration` flag, or one constructor with a bool, is the
`eq-v2` step 01 fault in miniature — one thing standing for two semantics, with
nothing able to report a caller that picked wrong. `04-guard.md` puts it
directly: **require a deliberate act, not a correct value.** Two types make the
act unavoidable, because you cannot hand a duration to something that prints
positions.

```rust
// mooloop-core/src/time.rs

/// A transport position. Bars and beats count from one, ticks from zero,
/// because that is what a musician reads off a transport.
pub struct BbtPosition { pub bar: u32, pub beat: u32, pub tick: u32 }

/// A length. Everything counts from zero: one bar is `1:0:0`.
pub struct BbtDuration { pub bars: u32, pub beats: u32, pub ticks: u32 }
```

Both built with `from_ticks(Ticks, Ppq)`, both `Display` as `bar:beat:tick`.
Integer division and remainder only — no allocation, no float — because
`transport.rs` is on the audio thread.

This is also what finally gives `Ticks` a reader. It has had the three helpers
everyone needed since the beginning and no caller outside its own tests; the
constructors go through `Ticks::beat`, `Ticks::beat_in_bar` and
`Ticks::within_beat` rather than around them.

## The two derivations collapse into one

- **`session/engine.rs:706`** builds `TransportPosition` by hand. Keep the
  struct and the bridge shape exactly as they are — this is not a wire-format
  change — and fill `bar`, `beat` and `tick` from `BbtPosition::from_ticks`.
  The `step` and `playlist_ticks` fields stay where they are; the pattern-vs-song
  modulus above them is genuinely two cases, as its own test comment says, and
  is not what is being consolidated.
- **`engine/transport.rs:97`** `beat_in_bar()` returns
  `BbtPosition::from_ticks(...).beat - 1`, keeping its 0-based contract and its
  signature. Its callers — `RenderReport`, the executor, the bridge, the UI —
  do not change.

Both then derive from one place, which is the point: today they agree only
because two people hardcoded the same number.

## The format moves out of the toolbar

`toolbar.slint:483`'s `PositionReadout` owns `pad()` and the join. Lift them
into a `BbtText` component in `controls.slint`; `PositionReadout` keeps its
`bar`/`beat`/`tick` in-properties and its width and tabular monospace, and
draws a `BbtText`. The Buffer face then has something to reuse rather than a
second copy to write.

**The markup formats and never derives.** There is no new `RollMetrics`-shaped
global here and there should not be: Slint is handed three integers and prints
them. That keeps the whole 4/4 question on the Rust side, where the constant
lives.

This is `.slint`-only — no new property crosses `main.slint` into `lib.rs` — so
it iterates on `scripts/slint-sketch` at about two seconds, not on an
eight-minute `mooloop-ui` build. Per `03-fix.md`'s cost table, do not batch it
with anything that does cross.

## Done when

- `BbtPosition` and `BbtDuration` exist, are `Display`, and go through `Ticks`.
- Both readouts derive from `BbtPosition`; no hand-rolled bar/beat arithmetic
  is left outside `time.rs`.
- `PositionReadout` draws a `BbtText` and looks identical.
- `BbtDuration` has no caller yet. That is expected — `buffer-implementation/03`
  step 5 is the caller — but **the guard in step 03 covers it now**, so it does
  not ship as another table with no reader.
