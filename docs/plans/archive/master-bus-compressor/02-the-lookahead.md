# 02 · The safety limiter looks ahead when asked

MOO-169. **Crate:** `mooloop-dsp` (`src/output_guard.rs`, Mixer & Routing's).

## What it builds

`OutputGuard::set_lookahead(ms)`, 0 to `MAX_LOOKAHEAD_MS` (5 ms), and
`OutputGuard::latency_frames()`. The delay ring is allocated when the guard is
built, for the longest lookahead at that rate, so setting it never allocates.

- **At 0 the guard runs the code it runs today.** Not a ring of length zero:
  the same branch, so the MOO-93 guarantees and their tests stand unchanged,
  and `a_signal_under_the_ceiling_passes_bit_identical` is joined by a test
  that a guard which was set to 3 ms and back to 0 matches a fresh one bit for
  bit.
- **Above 0** the detector reads the incoming frame and the output is the
  frame L behind it. An over arriving at the input starts a linear ramp of the
  reduction that reaches what that frame needs by the time the frame leaves,
  so its leading edge is turned down rather than shaped. The ramp takes the
  larger of the slope it is on and the slope the new over needs, which is what
  keeps every frame inside the window covered, and the hold is counted from
  when the over *leaves*. The final clamp stays as a floating-point backstop.
- Below the ceiling it is a pure delay: the output is the input L frames
  later, bit for bit.
- The scrub still runs on the way in, so the ring never holds a NaN.

## Tests that pin it

- 0 ms is bit-identical to today's guard, including after a round trip
  through a nonzero setting.
- At 1.5 ms a signal under the ceiling comes out exactly L frames late.
- At 1.5 ms nothing leaves above the ceiling, and **no output frame needed the
  final clamp** by more than an ulp: the ramp arrived in time. An over's first
  frame is ducked, where the zero-latency guard shapes it.
- Linked sides, NaN scrub and the adopt path keep working with a lookahead
  (`adopt` carries the ring when the lookahead matches).
- `latency_frames()` is `round(ms * rate / 1000)`.

## Changing it while playing

The delay's length changes, so the output jumps by up to 5 ms: a small
discontinuity, once, when the knob moves. Recorded rather than crossfaded --
it is a setting, not a performance control, and a crossfade would put two
copies of the mix in the output for its duration.
