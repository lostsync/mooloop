# 04 — An install keeps the transport running

The interim fix `LOOSE_ENDS.md` ("Every structural edit stops the song")
names, taken separately because it is small and audible on its own, and
because step 05 is easier to judge once this one is in.

## Build

- `RenderState::load_project` calls `transport.stop()`, and
  `install_project_in_ui` sets `playing` false and moves the playhead to zero.
  For a **structural edit** (`ProjectEdit` with `edit: Some(..)`), carry the
  playing state and position across the install instead. A project **open**
  still stops and rewinds.
- The install is prepared on the control thread and swapped in on the audio
  thread, so the position has to be read at the swap, not when the install is
  prepared, or the song jumps back by however long the preparation took.
  Carry a flag in `PreparedProject` and let the executor copy the outgoing
  transport into the incoming state at the moment it switches.
- Tails are still cut. That is step 05's problem, and the status file should
  say so plainly.

## Test

An executor-level test: playing, with the position past zero, install a
reordered project with the structural flag set. After the swap the transport
is still playing, at the old position plus the frames rendered since. With the
flag clear, it is stopped at zero.

## Acceptance

Listen: move a channel while a song plays. The song keeps going; you may hear
a dropout where the tails are cut, and that is expected until step 05.
