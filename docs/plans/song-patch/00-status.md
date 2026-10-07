# song-patch — status

Linear: project [Song patch](https://linear.app/mooloop/project/song-patch-3daef6dfe896).
Release label `0.1.7` (Adam, 2026-10-06: keep it in 0.1.7; 0.1.7 is tagged
when the patch lands).

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

Planned 2026-10-06.

## What each step found

**01 (MOO-520).** The patch is in the song file: boxes have `at` and
`open`, `SongModulation` has `tags` and `wires`, the five kinds and three
tag kinds declare their jacks (`core/src/patch.rs`), songs saved by song
modulation convert on load, integrity repairs wires and tags, and the
session has the verbs. Where it differs from the step file:

- **Tags are a list of their own** (`SongModulation::tags`), not
  `ModulatorParams` variants. A variant would have added an arm to every
  `match` on the params across five crates for a box with no DSP; the ids
  share one mint, so a wire names a box or a tag the same way.
- **`input` survives as a view.** `SongModulation::input_of` reads the wire
  into a box's input inlet back as the old single input, and `set_input`
  writes one, so the engine's compile, the shelf's input picker
  (`module_input_options`, `set_module_input`) and channel presets keep
  working unchanged until step 02 compiles the graph and step 03 replaces
  the picker.
- **Routes do not name an outlet yet, and have no `at`.** No box has a
  second outlet before step 04, and the canvas that draws assignment tags
  is step 03; both fields arrive there with serde defaults, so the format
  only grows.
- **`arm` still takes a source, not a `Jack`**, for the same reason; step
  03 changes it with the canvas.
- **Undo is the window's.** The verbs (`add_patch_box`, `move_patch_nodes`,
  `remove_patch_node`, `connect_patch`, `disconnect_patch`,
  `add_patch_tag`) each return whether they changed the song; step 03 wraps
  each gesture in `with_gesture_history`, a multi-box drag being one call.

**02 (MOO-521).** The engine runs the patch as a graph.
`CompiledModulation::compile` gives every box its inlets and every node a
tick order (Kahn's, tags first, ties in list order); a loop is broken at the
first box in list order still waiting, and that wire reads the previous
tick. A control wire carries a value and the tick's events. Where it
differs from the step file:

- **A wire can be saved late** (`Wire::late`). The step file's tie-break
  alone could not keep a converted Math box that read a module listed after
  it one tick late: topological order would run its source first. Every
  path that still speaks in single inputs (`SongModulation::set_input`:
  conversion on load, channel presets' racks, the shelf's picker) marks a
  wire from a module listed at or after its reader late, so those songs
  keep the list-order rule. A wire made on the canvas is not late; only a
  loop makes one late, at compile time.
- **The wire's trigger is the gate tag's events, not a bare `fired` flag.**
  An Envelope's gate counts NoteOns, NoteOffs and chokes, and a bare flag
  would lose the restarted release a stray NoteOff after a choke gives
  today. Trigger inlets fire on a NoteOn or a rise through 0.5, as planned;
  an Envelope fed by any other wire gates by its level.
- **Modes still gate the triggers.** The LFO's `retrigger` acts only with
  Retrigger on, the Step's `advance` only in Note mode, the Random's
  `trigger` only in Note mode, as today. Whether a wire should switch the
  mode by itself is a question for step 03's faces.

Null test (`engine/tests/modulation_null.rs`, release): every release song
plain, with the busy rack, and with a new rack where every kind hears
overlapping notes (with the starter kit's hat choke and a Math reading a
later slot) renders sample-identical to `main` at 8c036cd, largest
difference 0, 21 renders.

Cost of the control pass alone (`block_cost.rs` `patch_control_cost`,
release, this container, median of 2,000 128-frame blocks):

| Boxes | No wires | Chain | Fan |
| --- | --- | --- | --- |
| 64 | 5.2 µs (0.19%) | 3.6 µs (0.14%) | 3.5 µs (0.13%) |
| 256 | 20.6 µs (0.77%) | 14.8 µs (0.56%) | 13.6 µs (0.51%) |
| 1024 | 82.3 µs (3.09%) | 59.4 µs (2.23%) | 54.8 µs (2.05%) |

The chain and fan are Math boxes after one LFO, and cheaper than all-LFO;
the graph walk itself is not where the time goes. A thousand boxes is 3% of
a 128-frame block, so step 04's arithmetic boxes can each be a whole
module.

**03 (MOO-522).** The Modulation pane shows the patch. Adam said go on the
mock-up (2026-10-07, *"looks great"*). `ui/patch-canvas.slint` draws what
`ui/src/patch_canvas.rs` lays out: boxes spelled in the mono face (`lfo`,
`* -0.5`), tags as arrows, an assignment tag per route from a box, and one
`Path` per wire, routed orthogonally with rounded corners (the prototype's
`route` and `rpath`). One `TouchArea` reports press, move and release; Rust
hit tests jacks, then nodes, then wires. Drag between jacks to wire (a
wired inlet's wire is replaced, a wire pulled off its inlet onto nothing
comes out), click an outlet to arm Assign, click an inlet to pick its feed,
marquee and drag to move, Delete to remove; each is one undo step
(`patch_canvas_tests.rs` drives them through the window). Where it differs
from the step file:

- **Assignment tags are placed through `SongModulation::route_places`**, by
  source and destination, saved as an `at` on each route; `ModRoute` is
  copied by the realtime path and stays as it was. A route nobody placed
  stacks under its box.
- **Routes from a channel's outlet or keyboard have no tag yet.** Nothing on
  the canvas stands for those sources until step 06's song inlets; the
  route list beside the surface still shows them.
- **Arming is still by source, not by `Jack`**: no box has a second outlet
  before step 04.
- **The selected box's surface stays under the canvas**, not beside it as
  the mock-up drew it: today's surface is about 930 px wide. The dock's
  default height for the Modulation pane went from 300 px to 440 px so the
  canvas is about 235 px tall rather than 120; a saved layout keeps its own
  height. Step 05 opens faces in place and gives the canvas the pane.
- **Note wires are drawn solid in the note colour**: Slint's `Path` has no
  dash. The theme has no note colour of its own yet; the canvas uses
  `Theme.warning`.
- **The route dots' reveal** (song modulation's *Open*) is not in this
  step; it is MOO-531.

**04 (MOO-523).** Double-click empty canvas to type a box; double-click a
box to retype it. `mooloop-core/src/box_text.rs` holds the vocabulary,
`parse` and `spell`; a box keeps parameters and is spelled from them.
`counter`, `select` and `slew` are new `ModulatorParams` variants with
descriptors and DSP (`mooloop-dsp/src/modulator.rs`); the field and its
completion list are in `patch-canvas.slint`. The shelf's Add list is gone.
Where it differs from the step file:

- **Today's Math is the arithmetic boxes; nothing converts.** `+ - * /
  min max clip` are a Math module with that operator, which is what Math
  already stored, so a 0.1.6 song's Math module reads as its box with no
  conversion and the null test is untouched. Each gains an `operand` inlet
  (port 1) whose wire replaces the typed operand; `clip` has none.
- **Arguments read:** `lfo` takes a shape (`sin tri saw sqr rnd`) and a
  rate as a division (`1/4`, `1/8t`, `1/4.`) or in hertz (`2hz`); `step` a
  length; `slew` seconds or `ms`. A box spells back only the arguments that
  differ from its defaults, so a plain LFO still reads `lfo`.
- **An unknown box is outlined in red, not dashed** (Slint draws no
  dashes), and says so in the status bar when made rather than on hover.
  It saves its text (`text = "chord min7"`); a song from a later build with
  a `kind` this one does not know opens with that box unknown, its text
  kept, and a known kind it cannot read still refuses the song.
- **A typed box starts unwired.** The Add list gated a new box from the
  selected channel; typing does not.
- **Retyping to another kind** keeps wires on jacks of the same name and
  keeps routes when the new box has the outlet they read (`out`): an LFO
  retyped to `slew` keeps both, retyped to `counter` (outlet `index`) loses
  its routes.
- `MAX_INLETS` is 9: a `select 8` has `index` and `a` to `h`.

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
| 2026-10-06 | The patch stays in 0.1.7, rather than tagging 0.1.7 now and giving the patch its own release. |

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

## Open from September, not decided

- Reason's rack flip (Tab to the back of the rack) as a second view of the
  same cables (`README.md`, *Not in this plan*).
- Patterns played on conditions, and how that sits with Song mode.
- Wiring from a knob's right-click ("connect to…") as well as the Assign
  drag.
