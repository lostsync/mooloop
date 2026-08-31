# Node Model

Status: recorded direction, 2026-08-31. **Not scheduled, and not a plan.**

This document exists so a design conversation is not lost. It is not a
commitment to build any of it, and nothing in the active sequence depends on
it. `COMPOSABLE_DEVICE_UNITS.md` owns the unit contract; this owns the
product shape a node view would take *if* it happens.

## Why it is here at all

Adam's stated reason, and it is worth recording honestly: he likes how
node-based systems look and work, and he wants the option kept open. It was
not arrived at by discovering a need. That is a legitimate reason for a
personal instrument, and it is also why this is a direction rather than a
plan — a want that has not yet met a workflow should not reorder a roadmap.

The corollary matters more than the doc: **liking how node editing looks is
not the same as needing the graph architecture underneath it.** The visual
appeal — visible connections, signal you can watch move — is largely a UI
property. The expensive part is typed edges, buffer ownership, cycle policy,
and latency compensation. Before committing to the second, it is worth
checking how much of the first can be had on its own. A device that can
*display* its internal signal flow, even read-only, would test the appetite
at a fraction of the cost.

## The shape: devices are externals, the rack is the canvas

The distinguishing idea, and the answer to "how is this not The Grid".

Bitwig's Grid is a device you open, containing a blank canvas you build a
sound in. It replaces the instrument. Mooloop's devices would instead stay
**opinionated finished instruments — externals** — and the patching would
happen *around* them, directly in the ordinary device rack:

```text
piano roll -> [alternate velocity 64/127] -> ML-M1 -> [saturate] -> out
                        ^                                 ^
                   note objects                     audio objects
                                    LFO -> [x2, offset] -> ML-M1 cutoff
                                                ^
                                         control objects
```

Same primitives as a modular environment, opposite default. The Grid starts
empty; this starts as a working instrument you can unfold. `PRODUCT.md`
already rules out a Max/MSP-scale patching environment as the ordinary
workflow, and `FOCUS.md` already wants opinionated devices rather than
construction kits — so the differentiator is not a new position, it is the
one already written down.

## Three domains, three very different prices

The rack carries note, control, and audio, and a user-assembled object in
each costs a different amount. This is the most useful thing in this document.

| Domain | Example | Cost |
| --- | --- | --- |
| Note / event | Alternate velocities 64/127 before a synth | **Cheap** |
| Control | Modulator through user math into a knob | **Medium** |
| Audio | Split, detune, mix back — a hand-built chorus | **Expensive** |

- **Note** needs nothing new: serial, in order, no latency.
- **Control** already has sources, destination policy, rates and latency in
  `mod_metadata.rs`. It needs assemblable math objects and little else.
- **Audio** needs typed graph edges, buffer ownership, cycle policy, and
  plugin delay compensation.

The audio row is the catch. That example is a parallel path, and both
`FOCUS.md` and `AUDIO_ARCHITECTURE.md` say compensation is required before
parallel paths are trustworthy — it is explicitly deferred.

**If this is ever proven, prove it in the note domain first.** The velocity
alternator needs no new graph shape, no PDC, and no buffer ownership, and it
exercises the entire model end to end: an object in the rack, with a declared
note-in/note-out boundary, saved as a fragment and dropped on an existing
channel. If that feels good the direction is real; if it feels like ceremony,
that was learned for the price of one small device.

## The boundary contract is what makes fragments saveable

A thing built in the rack is a subgraph: objects, their wiring, their values,
and — critically — what it accepts and what it emits. Without a declared
boundary a fragment can only be stored as a whole channel, because nothing
knows where else it may legally go.

With one, the preset browser question answers itself: an insert point in the
rack *is* a known boundary, so the browser offers only fragments whose
signature fits there.

This is the same problem the ML-M1 factory bank already hit one level down —
see `docs/plans/preset-system/00-status.md`. It is also why the port
descriptors in `COMPOSABLE_DEVICE_UNITS.md` stop being speculative the moment
this direction is taken seriously: `FOCUS.md` says device outlets should be
pulled in by a demonstrated workflow, and saving a fragment is one.

## What is deliberately not decided

- Whether a node view is a separate editor, an expansion of a rack row, or a
  whole-channel view.
- Whether users author objects or only wire shipped ones.
- Whether the graph is ever dynamically rewireable at runtime, or compiled to
  a fixed topology per edit.
- Any visual design.

None of these needs answering to keep the option open. What keeps it open is
the three habits in `COMPOSABLE_DEVICE_UNITS.md`, "What we actually do now",
which are worth following regardless of whether this is ever built.
