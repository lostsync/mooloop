# 01 — A view change is not a seek

Read `00-status.md` first. This step fixes the bug Adam reported and nothing
else. It is deliberately small: it does not change what a Pattern-mode switch
sounds like, it removes the cost from the cases that were never owed one.

## The change

`EngineCommand::SetCurrentPattern` (`crates/mooloop-engine/src/render.rs:4507`)
currently pays the full discontinuity price unconditionally:

```rust
EngineCommand::SetCurrentPattern(pattern) => {
    let from = self.automation_position();
    self.sequencer.set_current_pattern(pattern as usize);
    self.seeked = true;
    self.restore_lanes_left_behind(from);
}
```

It should pay it only when the selection actually changes what is scheduled.
Three conditions, all of which are cheap to answer and none of which needs
per-channel state:

1. **The selection moved.** `Sequencer::set_current_pattern`
   (`sequencer.rs:74`) silently ignores an out-of-range pattern and happily
   re-assigns the one already selected. Both cost a full choke today.
   Re-selecting the current pattern is not rare — the jump menu's current
   entry and the stepper bouncing off a bound both do it. Make it return
   `bool`: whether `self.current` actually changed.
2. **The mode reads the selection.** Only `PlaybackMode::Pattern` schedules
   from `current`. In Song mode nothing that is scheduled has changed, so
   neither the choke nor the lane restore is owed. This is the whole of
   Adam's report.
3. **The transport is running.** In Pattern mode with the transport stopped,
   nothing sequenced is sounding, so there is no note whose note-off has been
   stranded. What may be sounding is an audition or a held MIDI note — and
   under Adam's rule those should survive somebody looking at another
   pattern.

Condition 3 contradicts a comment that is currently in the tree, and the
contradiction is worth stating rather than quietly resolving.
`release_all_voices` is called outside the `playing` arm (`render.rs:5427`)
with the reason *"a seek while stopped still owes the release, for auditioned
notes if nothing else"*. That reasoning is sound **for a seek**: the user has
moved the playhead, and an auditioned note heard at the old position is
stale. It does not transfer to a pattern switch, where the playhead has not
moved and the note is a key the user is still holding down. So the `seeked`
flag keeps its stopped-transport behaviour and this command stops setting it
while stopped.

## What it deliberately does not do

**It does not narrow the Pattern-mode choke to the channels that changed.**
The obvious next thought is that a channel whose notes are identical in both
patterns should not be choked. Answering that needs the engine to carry which
channels had a voice started by the outgoing pattern — per-channel state
across blocks on the audio thread, which is exactly what
`restore_lanes_left_behind`'s own comment refuses for the analogous automation
case. Step 02 removes the reason the choke exists at all, which is a better
answer than making it cleverer.

**It does not stop the UI sending the command.** Not sending in Song mode
looks like a one-line fix in `on_pattern_selected` and would break recording:
`Sequencer::recording_tick` (`sequencer.rs:374`) reads `current` in both
modes, and in Song mode it is what decides which pattern a recorded note lands
in and where. The engine has to know the selection; it just must not charge
for it.

**It does not touch `restore_lanes_left_behind`'s Pattern-mode behaviour.**
That code is correct and its comment is one of the better ones in the file.

## Tests

Engine-level, in `render.rs`'s own test module beside the existing
`SetCurrentPattern` tests (`render.rs:6817`, `:6858`, `:8555`, `:8605`,
`:11526` — all of which run in the default Pattern mode and should stay
green). **Each new one must fail red before the fix**, and the assertion is on
the event lists rather than on audio, because `Event::Choke` is the thing
being counted:

- `switching_the_viewed_pattern_in_song_mode_chokes_nothing` — Song mode,
  transport playing, a sounding voice, `SetCurrentPattern`; assert no channel's
  `EventList` contains a `Choke`. This is the reported bug.
- `reselecting_the_current_pattern_chokes_nothing` — Pattern mode, playing,
  select the pattern already selected.
- `an_out_of_range_pattern_selection_chokes_nothing` — a selection the
  sequencer refuses must not cost anything either.
- `switching_the_viewed_pattern_while_stopped_chokes_nothing` — Pattern mode,
  transport stopped, an auditioned note sounding; it survives.
- `switching_the_playing_pattern_still_releases_every_voice` — the behaviour
  that stays. Written so that step 02 has to come and change it deliberately
  rather than discovering it.

A test that asserts silence would pass for the wrong reason — a `Choke` that
is emitted and then not reached is still a bug — so assert on the emitted
events, the way the existing seek and loop-fold tests do.

## Verification

Rung 2: `cargo test -p mooloop-engine`. The change is confined to one match
arm, one `Sequencer` method signature and its callers. Rung 4 before
committing, as always, and backgrounded.

Nothing in `docs/CURRENT.md` survives this unchanged: its Transport And
Arrangement section states that *"Every sounding voice is released at the loop
point, at a seek, and when the current pattern is switched under a running
transport"* (`CURRENT.md:765`). The last clause becomes *"…under a running
transport in Pattern mode"*, and that edit ships in the same commit.
