# 07 — notes through the patch

Note wires carry notes. A **Notes in** tag reads a channel's notes, a
**Notes out** tag plays notes on a channel, and between them every NoteOff
follows its NoteOn. This step builds the path with no note boxes yet (step
08): wiring Keys' Notes in straight to ML-M1's Notes out plays Keys' part on
ML-M1 too, in the same block, sample for sample.

This is new ground. Nothing transforms notes between the sequencer and the
instrument today except swing and choke groups (`README.md`, *What exists*),
so the step is Sequencing & Time's: it owns the note lifecycle
(`TEAMS.md:57`), and every rule below is about a note being released when
it should be.

## Where it runs

In `process_block_inner` (`render.rs:10109`), every channel's event list is
complete after `dispatch_auditions` (`:10262`): sequencer notes, owed
releases, loop-fold releases, chokes, and keyboard and audition notes.
Nothing has rendered yet. **The note pass runs there**, before the gate
table (`:10276`), so:

- a note sent to another channel arrives in the same block at the same
  offset, whichever channel renders first;
- a channel's gate tag (step 06) reads what the channel actually plays after
  the patch, not what its pattern asked for.

The pass walks note boxes in the compiled order (step 02), one control tick
at a time, so a note box that reads a control inlet (`chance`'s
probability, step 08) reads it **as of the previous control tick**, at most
32 frames earlier. Record that in `MODULATION.md`.

**A note wire cannot close a loop.** A note can't arrive a tick late
without moving it; the session refuses the wire (step 01's `connect`) and
says why.

## Notes in: copy or take

A Notes in tag has `take` (step 01's field), shown on the tag as a toggle:

- **Copy** (the default): the channel still plays its own notes, and the
  patch gets a copy. Keys plays, and ML-M1 doubles it.
- **Take**: the channel's NoteOn and NoteOff events are removed from its
  list and only the patch's output reaches anything. Chords and inversions
  on a channel's own part need this: Keys' notes in, `chord min7`, Keys'
  notes out.

Only notes are taken; parameter, bend and buffer events stay. A Choke on a
taken channel still reaches the channel.

## Note identity and release

**Every note a box emits gets a fresh id**, from a range the sequencer
(`(instance << 32) | note.id`), the keyboard (`u64::MAX - 128 - note`) and
auditions (`u64::MAX - note`) never use. Each box that emits keeps a table
from the input id to the ids it emitted; an input NoteOff releases exactly
those. A box that swallows a NoteOn (`chance`, step 08) remembers to
swallow its NoteOff.

Tables are fixed-size per box, built off the audio thread with the set and
sized like `SequencedVoices` (64, `voices.rs:29`). A NoteOn that finds its
box's table full is refused and counted, never half-played; this boundary
is documented beside the type and tested, as `CAPACITY_POLICY.md` requires.
Every NoteOn that reaches a channel's list also has to fit its 256 events;
the existing overflow counter (`event.rs:100`) covers it and the canvas's
Notes out tag shows a refusal.

**Release on every path that releases today** (`render.rs:8844-9214`):
- Stop, pause, pattern switch and mute release the source channel's
  sequenced notes, whose NoteOffs flow through the patch: nothing new.
- **Seek and panic choke** instead of releasing. A Choke on a Notes in
  channel releases everything the patch emitted from that channel, at the
  same offset. Panic also clears every box's table.
- **Editing the patch while notes sound**: removing a box or a wire, or
  rebinding a tag, releases the notes that went through it on the next
  block. The new set carries tables by box id (`carry_from`,
  `song_modulation.rs:97`), and a held note whose path is gone gets its
  NoteOff.
- A Notes out channel that is removed or reinstalled loses its voices as
  any channel does (`carry_strips_from`, `render.rs:7120`); the patch forgets
  them.

Seven sources release everything when the transport stops
(`sampler.rs:2208` and the rest); that stays as the backstop.

## Done when

- Keys' notes, copied and taken, reach ML-M1 at the same offsets they
  reach Keys, in realtime and in an offline render (an engine test that
  compares event lists).
- No note hangs after stop, pause, seek, pattern switch, mute, panic, a
  loop fold, or deleting the wire mid-note: one test each, checking the
  target's voices are all released.
- A full table refuses and counts; the boundary test passes.
- The note pass allocates nothing (the counting allocator,
  `AUDIO_ARCHITECTURE.md`, *Callback contract*).
- `LISTENING.md` has Keys doubled on ML-M1, and Keys taken and sent only to
  ML-M1.
