# 01 — One bar, one home

Shape A from `03-fix.md`: give the value one home and make every side read it.

Rust only. `cargo check -p <crate> --all-targets` is seconds; nothing here
crosses into markup.

## The home

```rust
// mooloop-core/src/time.rs

/// Beats to the bar. Four, everywhere, and this is the only place that says
/// so.
///
/// `Project::beats_per_bar` is persisted metadata and is **not** this value:
/// the audio thread is never given the project, so a signature it could not
/// read would be a signature it could not honour. Until that changes, the
/// format field is documentation and this constant is the behaviour.
pub const BEATS_PER_BAR: u32 = 4;
```

Put it in `time.rs` rather than `playlist.rs`, because `playlist.rs` is
arrangement data and this is a timebase fact. `time.rs` already owns `Ppq`.

## The eight call sites

Take them in this order; each is independent.

1. **`mooloop-core/src/playlist.rs:8`** —
   `STEPS_PER_BAR = STEPS_PER_BEAT as u32 * BEATS_PER_BAR`. Today it is a bare
   `16` sitting one line above `TICKS_PER_BAR`, which *is* derived. Change
   `STEPS_PER_BEAT` today and the two part silently.
2. **`mooloop-core/src/time.rs:50`** — `Ticks::beat_in_bar` reads
   `% u64::from(BEATS_PER_BAR)`. Drop *"assumes 4/4 for now"*; the constant's
   doc comment now carries it, in one place.
3. **`mooloop-core/src/sampler.rs:379`** — `frames_per_bar` derives from
   `BEATS_PER_BAR` rather than spelling `240.0`. Rewrite the doc comment: it
   currently claims to match "the convention the buffer device already uses",
   and after step 4 that is true by construction rather than by assertion.
4. **`mooloop-dsp/src/buffer_device.rs:119`** — call
   `mooloop_core::frames_per_bar` instead of its own `240.0`. **Keep the
   `.ceil()` and the `.max(4)`**; the ring's capacity must not shrink on a
   rounding change, and `with_capacity` has a floor for a reason. This is a
   construction-time path, not `process`, so there is no realtime question.
5. **`mooloop-engine/src/render.rs:2630`** — delete the private
   `BEATS_PER_BAR: f32 = 4.0` and convert at the one use site (`:4436`).
6. **`mooloop-engine/src/transport.rs:99`** — `rem_euclid` against
   `BEATS_PER_BAR`. Drop the second *"Assumes 4/4 for now"*.
7. **`mooloop-session/src/engine.rs:760`** — the test's `TICKS_PER_BAR / 4`
   becomes `TICKS_PER_BAR / BEATS_PER_BAR`. A test that hardcodes the number it
   is checking is the copy most likely to survive a sweep.
8. **`mooloop-project/src/integrity.rs:526`** — the repair compares against
   `BEATS_PER_BAR` rather than a literal `4`, and its message says what the
   field is: metadata the engine does not read. Do not change the repaired
   value or the `PROJECT_FORMAT.md` contract — **`03-fix.md`: do not widen a
   migration while fixing a default.**

## `project.beats_per_bar` becomes honest

Its doc comment in `mooloop-core/src/project.rs:743` currently says nothing
about who reads it. Say it:

> Time signature numerator, persisted and validated. **The engine does not
> read this.** Bar arithmetic everywhere uses `time::BEATS_PER_BAR`; this field
> exists so the format does not have to change when a signature can actually be
> honoured, and `integrity.rs` holds the two to the same value meanwhile.

That is `02-judge.md`'s "prefer the option that makes the code match claims
already written down" — except nothing was written down, so the work is to
write the honest claim rather than pick between two existing ones.

## Done when

- `BEATS_PER_BAR` is the only `4` in the workspace that means beats to a bar.
- `STEPS_PER_BAR` is derived, not spelled.
- `buffer_device` calls `frames_per_bar` and its ring is the same size it was.
- No comment anywhere says "assumes 4/4" except the constant's own.
- `cargo test --workspace` is green and **nothing on screen has changed**.
