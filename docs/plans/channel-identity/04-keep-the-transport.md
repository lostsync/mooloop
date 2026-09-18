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
not prepared on the control thread.** `RenderState::adopt_transport` became
`adopt_performance_state` for that reason -- one function whose rule is
*everything whose value is only correct at the instant of the switch*, so the
next such field has somewhere obvious to go. Both are position-in-time state whose
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

Two executor-level tests. The kept case renders four blocks, queues an install
and renders one more, then asserts the transport is still playing and *past*
where it was when the install was queued -- past, not merely non-zero, because
that is the half the flag exists for. The cleared case asserts stopped at
zero.

The kept one was verified failing against the tree with the copy disabled,
which is the same discipline `AGENTS.md` asks of a `dupe-audit` check: run it
before the fix or it is decoration.

Two more for the fields above, added 2026-09-18 when the copy was built.
`a_swap_carries_the_keys_that_are_down_and_the_notes_being_taken` is the
render-level one: a key down and a take note open, adopt, and the incoming
state holds both -- then the note-off that follows lifts the key and closes
the note, which is the thing that was actually broken.
`a_kept_install_carries_the_held_keys_and_a_cleared_one_does_not` is the
executor-level one, and it is there because the render test cannot see
whether the swap site asks: it also asserts the *cleared* case, since an open
that inherited a key from the song it replaced would be a new bug in place of
the old one.

Both were verified failing with the two copies removed, which is the same
discipline the transport test above was held to.

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
