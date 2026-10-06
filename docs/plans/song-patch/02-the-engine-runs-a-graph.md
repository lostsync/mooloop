# 02 — the engine runs a graph

The engine evaluates the patch as a graph: every box's inlets read the
wires into them, in a compiled order, every control tick. The pane still
shows the module grid (step 03 replaces it). After this step a song from
0.1.7 plays exactly as before, and a patch made in a test with a box
feeding another box's `rate` or `retrigger` plays as wired.

## Compiling the order

`CompiledModulation` (`core/src/modulation_plan.rs:95`) gains the graph:
- each `CompiledModule` lists, per inlet, the position of the box and the
  outlet that feeds it, or nothing. `gate: Option<u8>` and `reads:
  Option<u16>` (`:53-68`) go: a gate tag and a wire replace them.
- `compile` sorts boxes topologically (Kahn's order, ties broken by the
  boxes' order in the file, so the result is stable and a converted song's
  order is its old list order).
- **A loop runs one tick late.** A wire that closes a cycle is marked
  `delayed` and reads the previous tick's value of its outlet. This is
  `AUDIO_ARCHITECTURE.md`'s rule that feedback is an explicit delayed edge,
  and it replaces the "slot order" rule (`modulation.rs:1329`). The canvas
  draws a delayed wire differently (step 03).

The audio thread never sees boxes or wires: it walks the compiled order.

## What a control wire carries

**A value, and a trigger.** A control wire is an `f32` per tick plus a
`fired: bool`. A box that produces events sets `fired` on the tick they
happen: a gate tag sets it on every tick a NoteOn arrives, so two
overlapping notes still retrigger twice, as they do today
(`NoteGateEvents.note_ons`, `dsp/modulator.rs:190`).

A **trigger inlet** (`retrigger`, `advance`, `reset`, `trigger`) fires when
its wire's `fired` is set, or when its value rises through 0.5. An
LFO wired to an `advance` advances once a cycle; a gate tag advances once
per note.

The Envelope's `gate` inlet is held while the value is at or above 0.5 and
restarts its attack on `fired`. A gate tag's value is 1 while any note is
held on its channel, so the Envelope keeps today's held-note behaviour
(`modulator.rs:235-259`) and its choke release.

A **value inlet** (`rate`, Math's `in`) reads the value. For `rate`, the
wire is added to the knob as a modulation of it in octaves, ±1 meaning ±2
octaves, as the prototype did. Math's `in` replaces its old input as it is.

Each kind's inlets come from step 01's jack table; nothing in the DSP names
a port by number twice.

## DSP

`ModulatorSet` (`dsp/modulator.rs:753`):
- `ModuleSpec` (`:714`) takes per-inlet sources instead of `gate` and
  `reads`;
- `tick` (`:879`) stops delivering gates to every module first. It walks the
  order, gathers each box's inlets from the outputs table (this tick's for
  boxes already run, last tick's for delayed wires), and runs the box;
- the outputs table gains the `fired` flags beside the values;
- a gate tag is a box that reads `gates[seat]` and writes its value and
  `fired`. It is the only place a channel's note counts enter the graph.

Sizing does not change: the set is built off-thread from the song and
replaced whole (`engine/src/song_modulation.rs:57`). Inlet storage is per
box, sized by the jack table, so a box with eight inlets (`select 8`, step
04) costs eight slots and nothing is reserved for boxes that don't exist.

## The null test

Every song in `project/tests/fixtures/songs/` and the 0.1.6 conversion
fixture renders sample-identical before and after this step
(`engine/tests/modulation_null.rs` has the harness). Add one song that uses
each kind with a note input, two overlapping notes, a choke, and a Math
reading a later module (the old one-tick-late case, which must now come out
the same through a delayed wire).

## Measuring

`block_cost.rs` measures the song set's control pass (MOO-170's sweep was
left owed by song modulation). Add the graph's cases: 64, 256 and 1024 boxes
in a chain and in a fan. Record the numbers in `00-status.md`; they decide
whether step 04's arithmetic boxes need anything cheaper than a whole
module each.

## Done when

- The engine runs boxes in the compiled order; `gate` and `reads` are gone
  from `CompiledModule` and `ModuleSpec`.
- The null test passes for every fixture and the new song.
- A test wires an LFO into another LFO's `rate`, a gate tag into a Step's
  `advance`, and a two-box loop, and checks the values tick by tick.
- The cost cases are measured and recorded.
- `MODULATION.md` says what a control wire carries, what a trigger inlet
  fires on, and that a loop runs a tick late.
