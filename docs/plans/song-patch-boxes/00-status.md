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

Planned and agreed 2026-10-19. Each step is worked in order.

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
