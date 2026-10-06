# song-patch — status

Linear: project [Song patch](https://linear.app/mooloop/project/song-patch-3daef6dfe896). Release label: Adam's call (asked
2026-10-06, see below).

| Step | Issue | Milestone |
| --- | --- | --- |
| 01 | MOO-520 | Boxes and wires |
| 02 | MOO-521 | Boxes and wires |
| 03 | MOO-522 | Boxes and wires |
| 04 | MOO-523 | Boxes and wires |
| 05 | MOO-524 | Boxes and wires |
| 06 | MOO-525 | Boxes and wires |
| 07 | MOO-526 | Note wires |
| 08 | MOO-527 | Note wires |
| 09 | MOO-528 | Patches you can keep |
| 10 | MOO-529 | Patches you can keep |

Each step is blocked by the one before it, except that 09 needs only 06.
Step 02 carries MOO-170's owed measurement. Step 03 carries song
modulation's owed *Open* item on route dots revealing the pane.

Planned 2026-10-06. Nothing built.

## Adam's rulings

| When | Ruling |
| --- | --- |
| 2026-09-23 | The rack should be one song-wide patching system: *"if you wanna do node based processing, maybe just draw some damn nodes like a grownup."* |
| 2026-09-27 | Its own pane; lean, with small knobs and readouts on the boxes. |
| 2026-09-27 | The graph makes notes (*"notes: yeah, definitely"*) and knows the pattern number and length. |
| 2026-09-27 | Curated vocabulary, not an open language; Pd later as one box at most. |
| 2026-09-27 | Keep today's Assign gesture; math boxes scale. |
| 2026-09-27, on the prototype | `select` takes a count; cable bends move by hand; cable activity is tunable. Otherwise *"i dont even know what i'd change from a design standpoint."* |
| 2026-10-06 | *"everything worked as intended. you can go ahead and do the plan."* |

## Defaults the plan picked, open to Adam

Each is a call the prototype or the September conversation left open.

- **One wire per inlet.** Two signals into one inlet are summed by a `+`
  box, so the patch reads the way it runs (step 01).
- **Loops are allowed and run a tick late**, marked on the wire (step 02).
- **A control wire carries a value and a trigger.** Trigger inlets fire on
  the trigger or on a rise through 0.5, so a gate tag retriggers on every
  note, as today (step 02).
- **Tags are made from the canvas's menu, not typed** (step 06).
- **The prototype's vocabulary sidebar is not built**; the typing list is
  the vocabulary (step 04).
- **Small boxes, not big ones.** The LFO sequence is `counter` plus
  `select` plus `lfo`s, not one `lfo-seq` box. A saved selection (step 09)
  is the way to keep a bigger one.
- **Notes in copies by default**; *take* is a toggle on the tag (step 07).
- **Note boxes read control inlets as of the previous control tick**, at
  most 32 frames early (step 07).
- **A note wire cannot close a loop** (step 07).
- **`chance` is seeded**, so a bounce plays what playback did (step 08).
- **The pattern tag in Song mode reads the topmost playlist row** playing
  at the time (step 06).
- **Which release this is.** 0.1.7 was the first stage of the modulator
  push and is done. This plan could stay under 0.1.7 (tagged when the patch
  lands) or become its own release, with 0.1.7 tagged now and the later
  labels moving up one. Asked 2026-10-06.

## Open from September, not decided

- Reason's rack flip (Tab to the back of the rack) as a second view of the
  same cables (`README.md`, *Not in this plan*).
- Patterns played on conditions, and how that sits with Song mode.
- Wiring from a knob's right-click ("connect to…") as well as the Assign
  drag.
