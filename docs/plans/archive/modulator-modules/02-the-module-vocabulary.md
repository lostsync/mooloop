# 02 — The module vocabulary

Three new kinds, chosen because each proves a different thing the grid
needs, and all three live in the control domain — the domain
`NODE_MODEL.md` prices as "medium: assemblable math objects and little
else". Every one is bounded, allocation-free, `Copy`, ticked on the
existing 32-frame subdivision, and reaches destinations only through
ordinary routes.

## Step — a little Matrix

A clocked pattern: up to 16 steps, per-step value in `-1..1`, length,
musical rate from the shared `ModTimeDivision` vocabulary, glide amount,
and a trigger policy (free-running from transport, or note-advance).
This is the Reason Matrix gesture at modulator scale, and it is the
first stepped/`SignalShape::Stepped` source, which exercises the
metadata `mod_metadata.rs` has been carrying unconsumed.

Per-step values are sixteen descriptor ids in one contiguous block plus
the scalar params. That keeps the whole kind inside the 01 contract —
no bespoke persistence, no second editor paradigm; the step editor is
one authored component that reads and writes the same ids.

## Random — S&H, probability, drunk

The LFO's `Random` wave is a hidden sample-and-hold; this promotes the
idea to a kind with room to be musical: clocked or note-triggered draw,
bipolar or unipolar range, **probability** (chance a draw actually
replaces the held value), quantize-to-steps, and a **drunk** mode where
each draw walks a bounded distance from the last value instead of
jumping. Deterministic per-render seeding, same as the LFO's S&H today.

## Math — the first patch cord between modules

Reads another slot's output as its input; applies `+`, `-`, `*`, `/` by
a constant operand, or min/max/clamp to a range; emits the result as an
ordinary source. This is the eurorack move — a module whose input jack
is another module — and it needs one new rule, stated now and enforced
in the tick:

**Modules evaluate in slot order within a control tick. A module reading
a lower slot sees this tick's value; reading itself or a higher slot
sees the previous tick's.** One sentence, deterministic, identical
realtime and offline, no cycle machinery. The input is a slot reference
today; when durable `ModSourceId` lands (03), it resolves the same way
route sources do.

Division clamps its operand away from zero. Everything clamps its
output to `-1..1` — the rack's convention, applied at the module edge
so a route never sees an out-of-convention value.

## Explicitly not in this step

- Polyphonic RNG — needs per-voice modulation context that only ML-P8's
  native system has; a channel-rack source is post-reduction by design.
- Note filter / note-domain objects — cheapest domain per
  `NODE_MODEL.md`, but they sit in the note path before generators, not
  in the modulator rack; that is its own small plan when taken.
- Envelope follower, sidechain, FFT anything — outlet-contract and
  audio-graph work, sequenced behind the control table.

## Done when

Each kind: appears in the add menu, edits through the 01 surface with
undo, previews on its tile, persists sparse and loads back, renders
identically offline, and has a DSP test pinning its shape (step
sequence advance, probability at 0 and 1, drunk boundedness, math
slot-order rule). A saved project using only LFO/envelope is untouched.
