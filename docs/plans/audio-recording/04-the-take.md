# 04 — A take becomes the channel's sample

## Needs an answer first

Open question 2 in `00-status.md`: what lands in the pattern.

## Build

**When a take ends**, the session:
1. Waits for the drain thread to close the file.
2. Loads it the way any file is loaded (`load_sample_at_path`, on a worker
   thread). The drain may already hold the frames, but loading the finished
   file means a take and a dragged-in file take the same path, with the same
   decode, resampling and description.
3. Applies it through `apply_loaded_sample`, into the channel that recorded
   it, **by `ChannelId`** if `channel-identity/` step 03 has landed, and by
   the load token otherwise.

**The note (recommended answer to question 2)**
- One `NoteEvent` at the take's start tick.
- Its length covers the take, clamped to the pattern.
- Root note: the sample's `root_note`.
- It goes into the pattern that was current when recording started, not the
  one current when the take is applied. The engine event carries that pattern.

**Undo**
- The take (sample plus note) is one undo step, and undo restores the
  previous sample.
- That relies on `ProjectSnapshot.samples`, which only pins a sample when an
  undoable edit is recorded while it is loaded. The survey could not find an
  undo entry for an ordinary sample load; check that first. If loads are not
  undoable, the take is where that stops being acceptable, because a take
  overwrites what was there.

**Save**
- A take that is still in the recordings folder is saved like any other
  referenced file. Under the recommended answer to question 5, it is moved
  into the project's assets instead, and the reference is updated.
- `PROJECT_FORMAT.md` describes the rule.

## Test

- **Session:** a finished take on channel 2 while channel 0 is selected ends
  up on channel 2, with the note at the reported tick, and undo restores the
  previous sample and removes the note.
- **Save:** a project saved after a take reloads with the take.
