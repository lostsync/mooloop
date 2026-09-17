# 04 — An install keeps the transport running

**Landed 2026-09-17.** The interim fix `LOOSE_ENDS.md` ("Every structural edit
stops the song") names.

## What landed

`PreparedProject` carries a `keep_transport` flag, and the executor copies the
outgoing renderer's transport -- playing, position, frames played -- into the
incoming one at the instant it swaps.

**It is a flag rather than a transport value, and that is the whole design.**
The install is prepared on the control thread and swapped in on the audio
thread, and the song keeps playing in between. A position captured at
preparation time would be however many blocks stale by the time it landed, so
the song would jump backwards by the length of its own install. Only the
executor is at the right place to read it. `Transport::adopt_running_state`
takes the position *and* `frames_played`, because the two are one answer: left
behind, the incoming transport would report the song as having just started
while its playhead sat two minutes in.

Tempo, sample rate and timebase are deliberately **not** carried: those belong
to the project being installed, and an edit may have changed the tempo in the
same gesture.

## Where the plan was too narrow

It proposed keying on `ProjectEdit { edit: Some(..) }` -- the three channel
edits. That would have left a track add, a track move, an effect preset load
and a sample load still stopping and rewinding the song, and all four go
through the same install. `LOOSE_ENDS.md`'s own list names them.

The distinction that matters is **edit versus open**, and it falls exactly on
the function boundary: every `ProjectEdit` is an edit, and the three other
callers of `install_project_in_ui` are the opens -- startup, new song, and
opening a document. So the flag is `true` for the `ProjectEdit` path and
`false` for the other three, and nothing has to inspect what kind of edit it
was.

**Undo and redo are included**, which the plan's rule would have excluded.
Undoing a channel delete mid-song is an edit to the song you are listening to,
and stopping the transport for it would be the same surprise this step exists
to remove.

## Tails are still cut

Every voice, delay line, reverb tail and compensation ring is still emptied by
any edit, on every channel, because the incoming renderer is a fresh graph.
The song keeps its place and its clock; what was ringing at the moment of the
edit is not carried across. That is [05](05-strips-by-id.md), and
`LOOSE_ENDS.md` stays open for exactly that part.

## Test

Two executor-level tests. The kept case renders four blocks, queues an install
and renders one more, then asserts the transport is still playing and *past*
where it was when the install was queued -- past, not merely non-zero, because
that is the half the flag exists for. The cleared case asserts stopped at
zero.

The kept one was verified failing against the tree with the copy disabled,
which is the same discipline `AGENTS.md` asks of a `dupe-audit` check: run it
before the fix or it is decoration.

## Acceptance

Not yet listened to. Move a channel while a song plays: the song should keep
going, and a dropout where the tails are cut is expected until step 05.
