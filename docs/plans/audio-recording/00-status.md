# Audio recording — plan status

**Written 2026-09-17. Nothing has landed.** This is `SCOPE.md` §2 item 3
(audio input) together with the audio half of item 5 (recording), in the shape
Adam settled on 2026-09-17.

## Adam's decisions, 2026-09-17

1. **Audio records into the sampler.** The sampler is where a channel's audio
   lives, and it is the only such place. mooloop 0.2 is not a DAW: *"you could
   work really hard and use it as one but it'd be kinda like recording a song
   in a sampler."* There are no per-track audio clips, and this plan must not
   add any. He expects long-form audio to be solved eventually by song-level
   automation, at least from a UX point of view, and that is not this plan.
2. **Recording goes to the selected channel and pattern.** Nothing more
   elaborate for now.
3. **One input menu.** The sidebar's IN row, which currently lists MIDI
   inputs, becomes the channel's input picker for both kinds. With an audio
   input selected, record-arm on that channel captures audio; with a MIDI
   input selected, it captures notes, as it does today.

This replaced the report's suggestion of a separate channel-level recording
tap. The tap bank may still be how the input reaches the recorder inside the
engine, but it is not something the user sees.

## What exists

- **MIDI capture works end to end**, with the two bugs below.
- **Nothing takes audio in.** JACK registers `out_l`, `out_r` and `midi_in`.
  Core Audio opens an output stream only. `Executor::process` has no input
  parameter.
- **Pieces to reuse:**
  - `AudioTapBank` (`render.rs`): a consumer already receives a buffer that
    was filled for it before its strip runs.
  - `AuxIn`: a level stage and a copy.
  - The Buffer device's rules for a large ring, allocated and resized off the
    audio thread (`BUFFER_ENGINE.md`).
  - `set_channel_audio`, which publishes a sample, with the old one going back
    through the reclaim ring.
- **Nothing writes a `SampleData` to disk**, and a saved project can only
  refer to a sample that exists as a file. So a take has to become a file.

## Before step 01: two bugs in the MIDI path this plan builds on

Found while surveying on 2026-09-17, and being fixed on
`fix/midi-routing-and-record-wrap`:

- `EngineHandle::install_project` never re-attached the MIDI routing cell, so
  after the first project install, per-channel MIDI input choices never
  reached the engine.
- In pattern mode, the transport's position is never wrapped to the pattern,
  so every note recorded after the first pass landed on the pattern's last
  tick.

Audio takes need the same answer to "where in the pattern does this start"
that the second fix gives notes.

## Steps

| Step | What | Rung | State |
| --- | --- | --- | --- |
| [01](01-input-in-the-engine.md) | Drivers deliver input; the executor carries it; input meters | engine; macOS unverified | not started |
| [02](02-one-input-menu.md) | `ChannelInput`: MIDI or audio, one picker, saved | core, project, session, UI build | not started |
| [03](03-capture.md) | Bounded capture on the audio thread, drained to a WAV file off it | engine, session | not started |
| [04](04-the-take.md) | A finished take becomes the channel's sample, with a note and an undo entry | session, UI | not started |
| [05](05-interface.md) | Record button, input meter, monitoring, the non-sampler rule | UI build | not started |

## Open questions for Adam

Each of these changes what a step builds. They are listed here so the step
that needs an answer can stop and ask rather than guess.

1. **An audio input on a channel that is not a sampler** (a synth, DS-01,
   Aux In). Options: list audio inputs only on sampler channels; list them
   everywhere and switch the source to a sampler on the first take; or list
   them and refuse to record, with a reason. *Recommended: list them only on
   sampler channels, and have the IN row say why the rest are missing.*
   Needed by step 02.
2. **What lands in the pattern.** A take that only replaces the sample must be
   triggered by hand before you hear it again. *Recommended: the take plus
   one note at the tick where recording started, as long as the take, so
   playback reproduces what was played.* Needed by step 04.
3. **What a loop pass does.** Recording while the pattern loops: one take per
   arm (stop at the loop end), keep only the last pass, or keep all passes as
   slices. *Recommended for the first version: one take per arm, which ends at
   the loop point.* Needed by step 03.
4. **Monitoring.** Does an armed audio channel play its live input while you
   record? Without that you can't hear yourself unless JACK routes the input
   around mooloop. Needed by step 05.
5. **Where a take lives before the project is saved.** *Recommended: a
   recordings folder under the app's data directory, moved into the project's
   assets on the next save and deleted once nothing refers to it.* Needed by
   step 03.

## Not in this plan

- Per-track audio clips and anything else DAW-like (decision 1).
- Recording more than one channel at a time.
- Separate input ports per MIDI device under JACK (`CONTROL_SURFACES.md`).
- `#7`, the full `AudioBackend` boundary. Step 01 explains why it is not a
  prerequisite, and what would change that.
