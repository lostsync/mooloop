# song-patch-boxes — status

Linear: project [Song patch boxes](https://linear.app/mooloop/project/song-patch-boxes-8349d7cfacab).
Release label `0.1.7` (Adam, 2026-10-19: keep tweaking the patch before
tagging).

| Step | Issue |
| --- | --- |
| 01 · num and hold | MOO-533 |
| 02 · Step drawn as bars, and table | MOO-534 |
| 03 · switch and gate; gate becomes held | MOO-535 |
| 04 · Note boxes with many jacks | MOO-536 |
| 05 · wait | MOO-537 |
| 06 · note | MOO-538 |
| 07 · arp | MOO-539 |
| 08 · The contract and two example patches | MOO-540 |

Planned and agreed 2026-10-19. Each step was worked in order; all eight
were done by 2026-10-20 and the plan archived.

## What each step found

**01 (MOO-533).** `num` and `hold` are control boxes (`ModulatorParams::Num`,
`ModulatorParams::Hold`), and a num box shows its value on the canvas
(`PatchNodeRow::readout`, refreshed with cable activity). Where it differs
from the step file:

- **A num's value is a position across its range** (`ModNumParams::position`),
  not a number, so its knob and a route onto it sweep the range typed
  (`num int 0 16`). Typing one number sets the value (`num 0.25`); the
  spelling keeps the format and range, not the value, which the readout
  shows.
- **Routes clamp their source to `-1..1`.** A num may send more on its wires
  (so `num int 0 16` into `/ 16` works); an assignment reads every source
  clamped, in the engine (`SongModulator::output`, the box-knob path in
  `ModulatorSet::moved_knobs`) and in the UI's mirror
  (`CompiledModulation::offset_for`).
- The format is a stepped knob on the face (float, int, note); low and high
  are typed only.

**02 (MOO-534).** A Step's steps and a table's values are bars on the open
face (`patch_face::bars`, `PatchBarsRow`), drawn by dragging, one undo step
per stroke; the playing step or the slot read is lit from a new per-module
position the engine publishes (`ModulatorSet::position`,
`ModulatorMeters::publish_positions`, `Session::read_modulation_positions`).
The Step's sixteen value knobs left its face. Where it differs from the
step file:

- **Unipolar is a switch, not a rescale** (`STEP_PARAM_UNIPOLAR`, id 20,
  after the value block). Steps keep `-1..1` storage; unipolar plays a
  negative step as zero and draws `0..1` from the floor, so switching back
  loses nothing.
- **A table's wire writes are not saved.** Only drawn values are params; a
  write is runtime state, as a counter's count is. Drawing a value again
  replaces what a wire wrote there.

**03 (MOO-535).** `switch n` (`ModulatorParams::Switch`, outlets `a` to `h`
as many as it has) and `gate` (`ModulatorParams::Gate`) on control wires;
today's note-reading `gate` is now typed and shown as `held`. Where it
differs from the step file:

- **The new gate is saved as `kind = "pass"`.** A saved `kind = "gate"` is
  the held box, so a song saved before the rename opens unchanged; only the
  text and label say `held`.
- **A gate passes events as well as the value.** An open gate fed by a gate
  tag lets its NoteOns through to an Envelope or a counter; with nothing
  wired into `open` it is closed.
- A switch sends values only, not events, on whichever outlet it picks; a
  route from a switch reads outlet `a`.

**04 (MOO-536).** `pick`, `unpack n` and `merge n` (`ModulatorParams::Pick`,
`Unpack`, `Merge`), and `switch` and `gate` on notes. The note pass now runs
a graph of boxes rather than a tree from each tag: each box has up to eight
note inlets (`CompiledNoteBox::inputs`, a `NoteFeed` each) and writes its
notes tagged with the outlet they leave by. Where it differs from the step
file:

- **A switch's or a gate's sort is a param** (`notes`), set when a wire of
  the other sort comes into `in` (`SongModulation::connect`) and kept when
  the box is retyped. Wires out of it that no longer fit go.
- **`pick` is counted from 1 and -1** (`pick 2`, `pick -2`), its knob a
  gapless position (0 the lowest, -1 the highest).
- **A note switch's `which` and a note gate's `open` are read a block
  late**, as a transpose's semitones are, so a note at the very first block
  after play sees them as they stood before.

**05 (MOO-537).** `wait` (`ModulatorParams::Wait`): a note lane in the note
pass (`WaitQueue`, 128 events for 64 tracked notes, on a frame clock the pass
keeps) and a control lane in the set (`ModulatorSet::rings`, one 2048-reading
ring per wait). Where it differs from the step file:

- **Beats are typed as the division that long** (`wait 2b` is `wait 1/2`);
  the settings are an LFO's: a time in ms, or a division when synced. A
  beat count with no division (`wait 5b`) is refused.
- **The control ring is decimated**, a reading every few control ticks so 8
  seconds fits at any sample rate, and read between readings.
- The delay is taken at the tempo when each note arrives.

**06 (MOO-538).** `note` (`ModulatorParams::Note`): the set logs each tick
its trigger struck or let go in a block (`ModulatorSet::note_strikes`, up to
16; `begin_block` empties them), and the note pass plays them in the next
block at their ticks' offsets. A note box is a root of the note graph with
no notes-in tag (`CompiledNoteBox::root` is `None`). Where it differs from
the step file:

- **A transport tag into a trigger fires once a beat.** Its ramp rising
  through 0.5 fired a trigger a second time mid-beat (a counter on Beat
  counted twice a beat); a transport tag's signal is now `clocked`, and a
  trigger takes only its `fire`.
- **A clocked trigger never falls**, so its note lasts until the next one;
  a level trigger (a `held` box's gate) ends the note as it falls.
- Unwired, pitch is middle C and velocity 100.

**07 (MOO-539).** `arp` (`ModulatorParams::Arp`: mode, division, octaves,
gate). The note pass keeps each arp's chord (`ArpState`) and steps it on the
transport's grid inside the block, from the block's spans in beats
(`NoteClock::beats`), or on its `advance` as the set logged it last block
(`ModulatorSet::note_strikes`, as a note box's trigger). Where it differs
from the step file:

- **A wired `advance` replaces its own clock**, a block late as every
  control inlet a note box reads; `reset` works either way.
- **The gate is a share of the rate's step** even when `advance` clocks it,
  and the next step always ends the note before it.
- **A note and a step at one offset: the note is in the chord first**, so a
  chord played on the beat sounds on that beat's step and a chord let go on
  it does not.

**08 (MOO-540).** `MODULATION.md` describes every box the plan added. The
left hand patch and the arp from parts ship as patch presets
(`mooloop_core::patch_factory`, seeded by `mooloop_project::seed_patch_bank`
into the patch presets' directory at start, once). Where it differs from
the step file:

- **Their tags land empty**, hinted Notes in, Notes out and Beat, as any
  patch preset's do: a preset names nothing of the song it lands in, a
  transport tag included.

