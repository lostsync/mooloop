# Task: Channel Buffer Device — Stage 1

Implement the first working buffer device for mooloop. `docs/BUFFER_ENGINE.md` is
the background hypothesis; this document supersedes it wherever they disagree.
Read both, then follow this one.

## What it is

A retained-audio insert device that sits at an arbitrary position in a channel's
ordered device chain. It continuously writes its input into a bounded stereo
circular buffer and reads back out of that buffer. When idle it is a wire. When
an event fires, the read head detaches and plays some other part of the recent
history — jumped, reversed, rate-shifted, repeated — and then returns to live.

The target idiom is Max/MSP `buffer~` + `groove~` driven by control events: the
IDM percussion-editing vocabulary (Squarepusher-style chopping, stutter,
reverse). It is **not** a turntable emulation and **not** a beat-repeat effect.

## Stage 1 scope

Build the audio engine and a minimal way to fire events at it. Do not build the
note-mapping layer, the parameter-lock UI, or the preset system — those are
Stage 2 and are specified at the bottom for context only.

Deliverable: one insert instance, driven by hardcoded or debug-triggered events,
that passes the acceptance tests below.

## Hard requirements

### Follow is bit-transparent and zero-latency

Correction to `BUFFER_ENGINE.md`: Follow is **not** "minimal latency." It is
zero. Write the incoming block into the ring, then read from the position just
written. Same samples, same callback, no delay, no added latency, no
coloration. Bypassing the device must be sample-identical to running it in
Follow. Do not add a safety margin.

### The writer never stops

Input is always written to the ring, verbatim, regardless of read-head state.
There is no freeze mode, no pause-the-writer mode, and no path by which the
device's own output re-enters the ring. Explicitly out of scope: write feedback,
overwrite behavior, resampling the device output.

(Snapshot-for-project-save is a separate mechanism — a copy *out* of the ring
into a project WAV asset. Keep it distinct from anything to do with writer
state. Stage 1 may stub it.)

### Head model

```rust
struct ReadHead {
    position: f64,      // fractional frames, absolute ring index
    rate: f32,          // signed; negative is reverse
    // window / repeat state
}
```

Rate is applied **instantly**. No smoothing, no ramping, no inertia between the
previous rate and the new one. Sample-accurate hard edits are the point of the
device; smoothing destroys the thing being built. Momentum-based gestures
(tape stop, backspin) are a Stage 2 option and must not be baked into the head.

### Events carry a tuple, not a single value

One event fires atomically and sets all of:

| Field | Units | Notes |
|---|---|---|
| `offset` | beats, negative = back in time | how far behind the write head to jump |
| `rate` | ratio, signed | 1.0 = normal, -1.0 = reverse, 0.0 = hold |
| `window` | beats, optional | length of the looped region; `None` = play forward freely |
| `repeat` | count, optional | number of window repetitions |
| `duration` | see below | how long the detachment lasts |
| `crossfade` | ms, 0 allowed | applied at every discontinuity |

`duration` is one of: `Steps(n)` — hold for n sequencer steps; `UntilNextEvent`
— hold indefinitely until superseded; `Gate` — hold until the corresponding
note-off. Manipulation is **latching**, not gated by default. A detached head
keeps doing what it was told until told otherwise or until its duration expires.

Stage 1 needs the event struct and the engine's handling of it. It does not need
the sequencer or MIDI plumbing that will eventually produce the events.

### Units are beat-relative

`offset` and `window` are expressed in beats and converted to frames against
current tempo at event time. They must survive tempo changes and be meaningful
across patterns. Do not use raw sample counts in the event API.

### Collision behavior

The writer closes on any read head running slower than 1.0x forward:

- rate 1.0 forward — constant gap, no collision
- rate 0.0 (hold) — closes at 1x
- rate -1.0 (reverse) — closes at 2x
- rate 0.5 — closes at 0.5x

When the write head reaches the read position, **force an immediate return to
live** with the event's crossfade applied. Do not wrap (musically arbitrary
material) and do not clamp (silent stall). Log or expose the event so it's
visible during testing.

### Ring sizing

Size the ring in **bars**, allocated off the audio thread, resized only on
tempo/config change and never from the callback. Default to at least 8 bars.
At 48 kHz stereo `f32` that's roughly 6 MiB at 120 bpm — acceptable for an
opt-in insert. Expose the memory cost.

### Interpolation

Variable-rate reads require interpolation. Use Hermite/cubic minimum. Linear is
not acceptable — hard reverse jumps at arbitrary rates are the aliasing
worst case and this device does that constantly. Aliasing artifacts are not a
desirable characteristic here.

### Crossfade

Both the detached position and the live position exist in the ring
simultaneously, so returning to live is a two-read equal-power fade, not a
seek. Default 2–3 ms. Must be settable to zero — an intentional click is
sometimes the desired result.

### Realtime constraints

- No allocation, locks, file I/O, or large-object destruction in the JACK callback.
- Buffer allocation and resize happen off the audio thread; hand off via
  lock-free pointer swap; defer destruction until the graph releases old data.
- Deterministic behavior across varying block sizes.
- Defined behavior for ring wraparound, transport stop, tempo change, and
  project reload.
- Offline and realtime render paths use identical buffer behavior.

## Acceptance tests

1. **Transparency.** Null-test the device in Follow against bypass. Must be
   sample-identical, with zero added latency.
2. **Jump.** Fire an event with `offset = -1 beat, rate = 1.0`. Output is the
   input delayed by exactly one beat, indefinitely, with no drift.
3. **Reverse.** `offset = -2 beats, rate = -1.0`. Plays backward, collides with
   the writer at the predicted time, force-returns cleanly.
4. **Stutter.** `offset = -1/16, window = 1/16, repeat = 8, rate = 1.0`.
   Produces eight clean repetitions of the last sixteenth, then returns live.
5. **Return.** Every return-to-live in the above lands on genuinely live,
   undelayed audio — not a delayed stream that catches up.
6. **Declick.** With crossfade at 2 ms, none of the above produces audible
   clicks at discontinuities. With crossfade at 0 ms, the discontinuities are
   present and abrupt.
7. **Block size.** All of the above produce identical output at 64, 128, 256,
   and 1024 frames per period.
8. **RT hygiene.** No allocations or locks in the callback under any of the
   above, verified.

## Reject criteria

Stop and report rather than working around, if:

- Follow cannot be made transparent and zero-latency.
- The result is indistinguishable from a beat-repeat/stutter plugin — i.e. the
  event tuple's expressiveness doesn't survive into the audio.
- RT constraints force smoothing or latency that softens the edits.

## Stage 2 — context only, do not build

- MIDI note layer: keyboard zones (rate / position / gesture) and notes that
  fire stored event tuples, in the style of Tim Exile's The Finger.
- Per-step parameter locks in the pattern sequencer, Elektron-style, so a step
  carries an inline event tuple.
- Optional momentum gestures (tape stop, backspin) as an event type.
- Multiple read heads.

Design Stage 1's event API so these layer on top without restructuring the head.
