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

## Acceptance — listened to 2026-09-17

Adam moved a channel with a song playing. **Time is kept**; the song carries
on from where it was, which is what this step was for. Audio drops out for a
split second, and his reading was that it sounded like only the reordered
channel was affected.

**It is not.** Measured at the executor the same day
(`every_install_silences_every_voice_until_strips_are_kept`): a *null*
install -- the same project, nothing moved, nothing changed -- takes the
master to exact silence just as completely as a reorder does. Two channels
were holding notes and both stopped. A drum channel hides it because the next
hit arrives within a step, which is why it was hard to tell on one.

So the dropout is the fresh graph, not the reorder, and it is on every
channel rather than the moved one. That is what step 05 is for, and the
measurement is kept as a test so the change it makes is provable rather than
described: when strips survive an install, an install that changes nothing
should change nothing audible, and that test inverts.
