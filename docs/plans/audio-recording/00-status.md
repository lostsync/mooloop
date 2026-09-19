# Audio recording — plan status

Linear: [MOO-16](https://linear.app/mooloop/issue/MOO-16/audio-recording-one-input-menu-takes-go-into-the-sampler)
mirrors this file. [MOO-12](https://linear.app/mooloop/issue/MOO-12/audio-input) predates this
plan (same `SCOPE.md` item, filed before the migration) and is kept open as
the earlier tracking issue for the input side; MOO-16 is where the plan's
current shape lives.

**Written 2026-09-17. Steps 02, 03 and 04 landed 2026-09-18.** This is `SCOPE.md` §2 item 3
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

**The second round, 2026-09-18, after step 02 landed.** Adam, on reading it:

> *"any channel should accept it (there really shouldnt be "kinds" of
> channels...the channels are meant to be dumb slots) but if the device in
> the channel doesn't take audio, it just kinda...ya know, is useless. [...]
> we should be able to have many channels with an audio input. what if i want
> to record 2 things at once? or just for the simple fact that i dont want to
> have to set up the input every single time i go to a diff track to record
> something. it should just stay set up."*

> *"all i am trying to achieve right now is -- you kno whow recording works in
> ableton's clip mode? or bitwig's? [...] record audio into clips. the only
> place i have to house those clips currently is the sampler, so i want to
> build that workflow in there. maschine allows this almost exactly."*

5. **A channel is a dumb slot.** Any channel holds an audio input; a device
   that has no use for one ignores it. This withdraws step 02's non-sampler
   rule and open question 1's answer.
6. **Many channels hold an audio input, and it stays set.** Several can record
   at once. This withdraws step 02's one-recorder rule.
7. **Recording is clip recording, from the sampler's face.** No record-enable
   and not the global record-arm (which stays MIDI's): the sampler face gets a
   **Record page** with a live-updating waveform, like Buffer's, and its own
   record button.
8. **Clip mode.** On: a recording length is set -- 1 bar, say -- and the take
   stops by itself when it is reached. Off: it records until stopped.
9. **Pre-roll: a take starts on the next bar.** If the transport is stopped,
   pressing record starts it, so the rest of that bar is the count-in (there
   is no metronome yet).
10. **The audio input is its own row, AUDIO, beside MIDI IN**, and the two are
    independent. This withdraws decision 3's "one menu": with recording on the
    sampler's own button, a sampler recording audio still wants its own
    keyboard.

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
| [02](02-one-input-menu.md) | `ChannelInput`: MIDI, an app source (channel, track, master) or a hardware input; one picker, saved | core, project, session, UI build | **landed 2026-09-18**, UI drawing deferred to 05 -- see below |
| [03](03-capture.md) | Bounded capture of the chosen buffer at the end of each block, drained to a WAV file off the audio thread | engine, session | **landed 2026-09-18** -- see below |
| [04](04-the-take.md) | A finished take becomes the channel's sample, with an undo entry and no notes | session, UI | **landed 2026-09-18** -- see below |
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

## What step 02 actually did

> **Reworked the same day, after decisions 5, 6 and 10**: the two fields are
> independent rather than "one non-default", any channel may hold an audio
> input, any number may, and the routing is a per-channel table. The IN row
> went back to MIDI-only; the AUDIO row arrives with 05. What follows is the
> first version, kept for the record; "The rework" below is what is on
> `main`.

Everything but the markup, which the step itself sends to 05's contract pass.

- **Core** (`core/src/input.rs`): `AudioInputSource` (`Off`, `Master`,
  `Track(TrackId)`, `Channel(ChannelId)`; `Port` waits for step 01),
  `Channel.audio_input` beside `midi_input`, `ChannelInput` over the pair with
  the "at most one non-default" rule in `store`, `settle_input` for the
  non-sampler rule, `InputPicker` for the rows, and `audio_record_route`,
  which resolves ids to seats for the engine.
- **The menu** is the MIDI rows unchanged, a `── Audio ──` heading, then
  Master, `Track · <name>` for every track but the master, and
  `Channel · <name>` for every channel. The heading is a row -- `MenuField` has
  no separators -- and picking it changes nothing.
- **Session**: `set_channel_input` replaced `set_channel_midi_input`, and it
  holds both rules: only a Sampler takes an audio input, and picking one clears
  it from any other channel. `reset_channel_source` applies the non-sampler
  rule in the same edit, so an undo restores source and input together; so
  does loading a generator preset of another kind.
- **Engine**: `AudioInputRouting`, a per-generation cell like `MidiRouting`,
  carried by `InputState::audio_input` and attached in `install_project`.
  `an_install_carries_its_own_audio_input_routing` was checked against a tree
  that did not attach it. Nothing reads it on the audio thread yet.
- **Not built here, on purpose.** The greyed-out audio rows and a hidden CH
  row need new properties across `main.slint`, and the step batches that with
  05's. Until then the rows show everywhere and a pick on a non-sampler is
  refused with a status-bar message; a CH pick with an audio input is ignored.
  `InputPicker::is_enabled` is the rule the markup will draw.
- **Two things the step did not name.** A kit or channel document brings no
  audio input -- it would name a channel of another song -- and one is cleared
  on load. And the integrity pass repairs the three states the session cannot
  produce: an audio input on a non-sampler, both halves set, and a second
  recorder.

## The rework, 2026-09-18

What is on `main` after decisions 5, 6 and 10:

- `AudioInputSource` and `Channel.audio_input` are unchanged, and independent
  of `midi_input` -- neither resets the other. `ChannelInput` and the combined
  picker are gone.
- **No kind rule and no one-recorder rule.** `records_audio`,
  `settle_input` and the three integrity repairs that enforced them are
  deleted; any channel holds an audio input, through any change of device,
  and any number of channels do.
- `Session::set_channel_audio_input` beside a restored
  `set_channel_midi_input`, and `audio_input_taps`: every channel resolved to
  a seat, in bank order. The engine's `AudioInputRouting` is that table, still
  per generation and still attached in `install_project`.
- `AudioInputPicker` is the AUDIO row's menu -- Off, Master, the tracks, the
  channels -- waiting for step 05 to draw it. **The IN row is MIDI-only
  again**, exactly as before step 02.
- Kit and channel documents still bring no audio input.

## What step 03 actually did

- **Engine** (`take.rs`, `take_tests.rs`): a `Take` per recording channel, on
  its strip, with a `TakeStatus` the control side reads (phase, frames,
  dropped, start tick). `StructuralCommand::StartTake` arms one;
  `EngineCommand::StopTake` ends one. `advance_takes` runs every live take
  once a block at the read site `AUDIO_ARCHITECTURE.md` "Capture" describes.
  Tested against the rendered master bit for bit, with no allocation while
  recording; a one-bar clip is 96,000 frames at 120 bpm; a muted source
  records silence (checked against a tree without the gate -- the first
  version of that test passed without it, because a source that never
  sounded holds zeros anyway); two takes at once; a take rides its strip
  across an install.
- **Session** (`take.rs`): `TakeRecorder` arms a take -- opens the WAV, starts
  its drain, returns the command -- keeps a peak summary per 1024 frames for
  05's waveform, and `collect`s finished takes as `FinishedTake`s for 04.
- **Not done here, and where it goes.** Sending `Play` before a take when the
  transport is stopped is the caller's (05's button). No `EngineEvent`s were
  added: the plan named `CaptureStarted`/`CaptureEnded`, but the shared
  `TakeStatus` already carries all of it, and a second copy of the same facts
  on the event ring is the duplication `AGENTS.md` opens on. Hardware latency
  waits for step 01.
- **Master latency.** A channel's take is read after its compensation, so it
  lines up with the track it feeds; a take of the master is read after the
  master's own chain, so a latency-reporting device *on the master* delays a
  master take relative to a channel take by that device's latency. Nothing
  corrects for it yet; it matters only when both are compared.

## What step 04 actually did

- **The pump collects finished takes** from `UiState::takes` (a
  `TakeRecorder` over `recordings/` beside the settings file), decodes each on
  a worker through the same `load_sample_at_path` a dragged-in file takes, and
  `apply_take` puts it on the channel **found by `ChannelId`** -- selected or
  not, wherever it moved while the take ran. A take whose channel is gone is
  left in the recordings folder with a status-bar message.
- **Undo.** The step asked for a check first, and the answer was no: an
  ordinary sample load records no history. So `apply_take` records one entry,
  "Record Take", around `apply_loaded_sample`; undo puts back the sample that
  was there. Ordinary loads are unchanged.
- **Save.** The step wanted takes *moved* into
  `name.mooloop-assets/recordings/`. What landed is simpler and covers both
  save modes: a take is marked `embedded` -- owned by the song -- and a save
  now copies any owned sample that is not in the bundle yet into
  `samples/`, even in Referenced mode, where it used to become a reference.
  The original stays in the recordings folder for step 06 to find unused.
  `a_take_is_copied_into_the_song_in_either_mode` deletes the recordings
  folder after the save and reloads.
- **A damaged take** says so in the status bar when it lands.
- **Not yet exercised end to end.** Nothing arms a take until step 05's
  button, so the pump path is compiled and clippy-clean but first runs in
  05's acceptance.

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
- ~~Recording more than one channel at a time.~~ Withdrawn 2026-09-18
  (decision 6): several channels may record at once.
- Separate input ports per MIDI device under JACK (`CONTROL_SURFACES.md`).
- `#7`, the full `AudioBackend` boundary. Step 01 explains why it is not a
  prerequisite, and what would change that.
