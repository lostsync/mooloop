# Audio recording — plan status

Linear: [MOO-16](https://linear.app/mooloop/issue/MOO-16/audio-recording-one-input-menu-takes-go-into-the-sampler)
mirrors this file. [MOO-12](https://linear.app/mooloop/issue/MOO-12/audio-input) predates this
plan (same `SCOPE.md` item, filed before the migration) and is kept open as
the earlier tracking issue for the input side; MOO-16 is where the plan's
current shape lives.

**Written 2026-09-17. Nothing has landed.** This is `SCOPE.md` §2 item 3
(audio input) together with the audio half of item 5 (recording), in the shape
Adam settled on 2026-09-17, and since 2026-09-18 item 6 (resampling) as well,
which turned out to be the same feature with the source inside the app.

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

4. **Resampling is recording, with the source inside the app** (2026-09-18).
   *"its the same thing, only the source is different. the workflows would be
   identical."* So the input menu lists the app's own audio -- a channel, a
   track, the master -- beside the hardware inputs, and everything after the
   menu (capture, the take, the interface) is written once for both. Built
   internal-sources-first: that is the whole pipeline end to end, audible,
   with no driver work in it. Hardware input then arrives as one more source.
   `SCOPE.md` §2 item 6 is folded into this plan.

This replaced the report's suggestion of a separate channel-level recording
tap. The tap bank may still be how the input reaches the recorder inside the
engine, but it is not something the user sees.

## What exists

- **MIDI capture works end to end.**
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

Found while surveying on 2026-09-17, and **fixed on `main` the same day**
(`9d81e3a`, `6acf812`, then four follow-ups ending at `e293d29`):

- `EngineHandle::install_project` never re-attached the MIDI routing cell, so
  after the first project install, per-channel MIDI input choices never
  reached the engine.
- In pattern mode, the transport's position is never wrapped to the pattern,
  so every note recorded after the first pass landed on the pattern's last
  tick.

Audio takes need the same answer to "where in the pattern does this start"
that the second fix gives notes.

## Steps

**Worked in the order 02, 03, 04, 05, 01, 06** (decision 4). The files keep
their numbers so that references to them stay good; this table is in working
order.

| Step | What | Rung | State |
| --- | --- | --- | --- |
| [02](02-one-input-menu.md) | `ChannelInput`: MIDI, an app source (channel, track, master) or a hardware input; one picker, saved | core, project, session, UI build | not started |
| [03](03-capture.md) | Bounded capture of the chosen buffer at the end of each block, drained to a WAV file off the audio thread | engine, session | not started |
| [04](04-the-take.md) | A finished take becomes the channel's sample, with an undo entry and no notes | session, UI | not started |
| [05](05-interface.md) | Record button, source meter, the growing waveform, the non-sampler rule. **Acceptance: resample a loop into a sampler and play it back** | UI build | not started |
| [01](01-input-in-the-engine.md) | Drivers deliver hardware input into an input bus, which becomes one more source; monitoring | engine; macOS unverified | not started |
| [06](06-unused-takes.md) | Find and delete takes nothing refers to | session, project, UI build | not started |

**Why internal sources can go first without new scheduling:** capture is a
sink, not a consumer. It reads a buffer after the whole block has rendered,
and the take reaches a sampler only after it stops, so nothing inside the
block depends on it. That is why `OutletTap::Output`'s `TapIsLate` refusal,
which stops Aux In from hearing a channel's finished output, does not apply.
Every candidate buffer still holds its audio at the end of the block:
`ChannelStrip.bus`, each track's `BusStrip.bus` (`track_energy` in
`render.rs`'s tests reads it that way), and `master()`.

Step 03's prerequisite, a take that survives a structural edit, is met:
`channel-identity/05` carries channel strips (2026-09-17) and
`incremental-structure/02` carries track strips (2026-09-18).

## Open questions for Adam

Each of these changes what a step builds. They are listed here so the step
that needs an answer can stop and ask rather than guess. All five were
answered the day the plan was written, and are kept here as a record.

1. ~~**An audio input on a channel that is not a sampler.**~~ **Answered
   2026-09-17:** on a channel that is not a sampler, the audio inputs are
   listed but **greyed out**. If a channel with an audio input selected is
   switched away from Sampler, its input moves to the **no-input** row (Off).
   Step 02.
2. ~~**What lands in the pattern.**~~ **Answered 2026-09-17: nothing.** A take
   replaces the channel's sample and writes no notes. It is heard through
   whatever the pattern already triggers. Step 04.
3. ~~**What a loop pass does.**~~ **Answered 2026-09-17:** a take runs
   **until it is stopped**, straight through any number of loop passes. It is
   one continuous file, not one take per pass. Step 03.
4. ~~**Monitoring.**~~ **Answered 2026-09-17: a toggle.** Live input is heard
   only when the toggle is on. Step 05.
5. ~~**Where a take lives before the project is saved.**~~ **Answered
   2026-09-17:** a recordings folder. Adam added that takes nobody used need a
   way to be deleted, which is step 06.

Opened by decision 4, 2026-09-18. Each has a default the steps are written
to, so none of them blocks a step; Adam can overrule any of them.

6. **Where a channel or track is tapped.** Default: **after its fader and
   pan**, which is what you hear. A resample is "print what is playing", and
   a microphone is recorded as it arrives, so this is the reading that keeps
   decision 4's "the workflows would be identical" true. Pre-fader would be
   a second row per source, and nothing has asked for it. Step 02.
7. **Recording a channel into itself.** Default: **allowed.** Resampling in
   place is the ordinary gesture, and it cannot feed back: the take replaces
   the sample only once it stops. The same holds for recording a track that
   the destination channel plays into. Step 02.
8. **Monitoring an app source.** Default: **not offered.** You are already
   hearing it, so the toggle belongs to hardware inputs only. Step 05.

## Not in this plan

- Per-track audio clips and anything else DAW-like (decision 1).
- Recording more than one channel at a time.
- Separate input ports per MIDI device under JACK (`CONTROL_SURFACES.md`).
- `#7`, the full `AudioBackend` boundary. Step 01 explains why it is not a
  prerequisite, and what would change that.
