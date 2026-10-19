# 04 — typing a box

Double-click the canvas, type `select 4` or `* -0.5`, press Enter, and the
box is there. The curated vocabulary of control boxes lands with it. After
this step Adam's patch 4, the LFO sequence, can be made and heard.

## The vocabulary

The control boxes, each with its jacks in step 01's table. An argument
typed after the name sets a parameter; the box keeps it as a parameter,
not as text, and is spelled back from its parameters (`textFor` in the
prototype), so `*   -.5` becomes `* -0.5`.

| Box | Arguments | Inlets | Outlets | Notes |
| --- | --- | --- | --- | --- |
| `lfo` | shape, rate (`lfo tri 1/4`) | `rate`, `retrigger` | `out` | today's LFO |
| `env` | | `gate` | `out` | today's Envelope |
| `step` | length | `advance`, `reset` | `out` | today's Step, gains `reset` |
| `random` | | `trigger` | `out` | today's Random |
| `+` `-` `*` `/` | operand | `in`, operand | `out` | a wire into the operand inlet overrides the argument |
| `min` `max` | operand | `in`, operand | `out` | |
| `clip` | low, high | `in` | `out` | |
| `counter` | n, 2 to 64 | `advance`, `reset` | `index` | the index as 0 to 1 across n steps, as the prototype does |
| `select` | n, 2 to 8 | `index`, then n inputs `a` to `h` | `out` | Adam: *"i'd make select take an argument"* |
| `slew` | time | `in` | `out` | smooths jumps |

**Today's Math becomes the arithmetic boxes.** A Math module with
`Multiply` and operand 0.5 is `* 0.5`; `Add` is `+`, and so on; `Clamp` is
`clip` with its low and high. Its clamp to −1..1 stays on every arithmetic
box's output, as Math has it (`dsp/modulator.rs:662`). The conversion runs
on load, beside step 01's, and the null test (step 02) covers it.

Each new box is a `ModulatorParams` variant with descriptors, as the five
are. Whether each arithmetic box is a whole module or something cheaper is
decided by step 02's measurement; the format does not change either way.

**Not here:** note boxes (`chord`, `chance`, `scale`, `gate`) are step 08;
tags are made from the canvas edge, not typed (step 06).

## Typing

- Double-click empty canvas: a text field opens there, with a list below of
  the boxes whose names start with what is typed, each with one line saying
  what it does (the prototype's `newbox`). Up and Down move in the list, Tab
  completes the name, Enter makes the box, Escape cancels.
- **A name not in the vocabulary makes a dashed box** that does nothing,
  keeps its text, and says so on hover. It is saved, so a song from a later
  build with a box this one does not know opens with the box kept and
  dashed, not dropped. (Integrity keeps it; the engine skips it.)
- Double-click a box's spelling to retype it. Retyping keeps its wires on
  the jacks that still exist by name, and drops the rest.
- Retyping a box to a different kind is a new module with a new id; its
  routes follow it when its outlet still exists.

The Add list in the shelf goes. The vocabulary sidebar of the prototype is
not built: the completion list is the vocabulary (an illustrative part of
the mock-up; see `README.md`).

## Done when

- Every box above can be typed, saved, reopened and heard; each has a DSP
  test of its output for a known input.
- Math modules in the fixtures convert to arithmetic boxes and the null test
  still passes.
- An unknown name round-trips as a dashed box.
- `LISTENING.md` has the LFO sequence: a `counter 4` advanced by the
  transport's beat (step 06 gives the beat; until then by an `lfo`),
  `select 4` over four `lfo`s of different rates, to a filter cutoff.
