# 04 — Note boxes with many jacks (MOO-536)

- **`pick low|high|n|-n`**: keeps one note of each chord (the notes starting
  at one offset): lowest, highest, nth from the bottom, nth from the top.
- **`unpack n`**: outlet k carries each chord's kth note from the bottom.
- **`merge n`**: n note inlets into one stream. An inlet takes one wire, so
  this is how two streams meet at one notes-out.
- **`switch` and `gate` on notes**: a NoteOff follows its NoteOn to the
  outlet it went to; closing a gate releases what it let through.

## Plumbing

A note box with several note inlets or outlets, each outlet with its own held
table (`note_patch.rs`, `CompiledNoteBox`). A box whose jack sort follows the
first wire into `in` (`switch`, `gate`).

## Done when

modal → pick low → transpose -12 → merge with the chord plays a left hand
under it, and tests show every NoteOn released through each new box.
