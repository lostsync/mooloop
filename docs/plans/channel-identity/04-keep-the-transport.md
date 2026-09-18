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

### The transport is not the only thing the swap drops

Named here because `reports/fable-2026-09-18.md` found that this step did not
name them. A project install builds a fresh `RenderState`
(`crates/mooloop-engine/src/executor.rs:226`), and what crosses the swap today
is nine `SharedCells` plus `InputState { record_armed, midi_routing }`
(`crates/mooloop-engine/src/lib.rs:574-586`). Two further pieces of
performance state are built fresh at `RenderState::new` and carried by
nothing:

- `held_keys` (`render.rs:2911`, built at `:3043`) — which channels are
  holding each MIDI note down, so a release goes where the press went.
- `recording` (`render.rs:2919`, built at `:3045`) — the in-flight MIDI
  capture, one open note per pitch.

Today the loss is hidden by the very thing this step removes: every
structural edit stops the song, so the old voices are gone with the old
renderer anyway. **Once the transport survives the install, the loss becomes
audible** — a key held across a structural edit has its note-off delivered to
a renderer that never saw the press, and a note captured but not yet closed
is dropped from the take.

So this step carries both, and it carries them the same way it carries the
transport: **copied from the outgoing state at the swap, on the audio thread,
not prepared on the control thread.** Both are position-in-time state whose
value is only correct at the instant of the switch, which is the same reason
the transport position cannot be read when the install is prepared. Moving
them is copying a fixed-size array — no allocation, realtime-safe.

Deliberately *not* `InputState`. That struct is a hand-maintained list
prepared ahead of the swap, and it has needed patching three times this month
(record arm, then routing twice); adding fields to it that the swap alone can
fill would be the wrong mechanism as well as the wrong moment. If this step
is cut back to the transport alone, say so in `00-status.md` and say what
carries them instead — leaving it unstated is how record arm was lost.

## Test

An executor-level test: playing, with the position past zero, install a
reordered project with the structural flag set. After the swap the transport
is still playing, at the old position plus the frames rendered since. With the
flag clear, it is stopped at zero.

A second executor-level test for the two fields above: with a note held and a
capture open, install a structural edit; the incoming state still holds the
key on the same channel and still has the open note, and the note-off that
follows closes both. With the flag clear, both are empty.

## Acceptance

Listen: move a channel while a song plays. The song keeps going; you may hear
a dropout where the tails are cut, and that is expected until step 05.
