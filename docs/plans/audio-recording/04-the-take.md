# 04 — A take becomes the channel's sample

## Nothing lands in the pattern (Adam, 2026-09-17)

A take replaces the channel's sample and writes no notes. It is heard through
whatever the pattern already triggers. So `CaptureStarted`'s tick is used
only for alignment and display, not to place anything.

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

**Undo**
- The take is one undo step, and undo restores the previous sample.
- That relies on `ProjectSnapshot.samples`, which only pins a sample when an
  undoable edit is recorded while it is loaded. The survey could not find an
  undo entry for an ordinary sample load; check that first. If loads are not
  undoable, the take is where that stops being acceptable, because a take
  overwrites what was there.

**Save**
- A take that is still in the recordings folder is saved like any other
  referenced file. On save it is moved into the project's assets
  (`name.mooloop-assets/recordings/`), and the reference is updated, so a
  saved song never depends on the shared recordings folder.
- `PROJECT_FORMAT.md` describes the rule.

## Test

- **Session:** a finished take on channel 2 while channel 0 is selected ends
  up on channel 2, writes no notes to any pattern, and undo restores the
  previous sample.
- **Save:** a project saved after a take reloads with the take.
