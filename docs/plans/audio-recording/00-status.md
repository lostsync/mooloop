# Audio recording — plan status

Linear: [MOO-16](https://linear.app/mooloop/issue/MOO-16/audio-recording-one-input-menu-takes-go-into-the-sampler)
mirrors this file. [MOO-12](https://linear.app/mooloop/issue/MOO-12/audio-input) predates this
plan (same `SCOPE.md` item, filed before the migration) and is kept open as
the earlier tracking issue for the input side; MOO-16 is where the plan's
current shape lives.

**Written 2026-09-17. Steps 02-05 landed 2026-09-18, 01's JACK half 2026-09-19 and its Core Audio half 2026-09-20; step 06's quit prompt 2026-09-20 and the rest of it 2026-09-23.** This is `SCOPE.md` §2 item 3
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
| [05](05-interface.md) | Record button, source meter, the growing waveform, the non-sampler rule. **Acceptance: resample a loop into a sampler and play it back** | UI build | **landed 2026-09-18** -- see below |
| [01](01-input-in-the-engine.md) | Drivers deliver hardware input into an input bus, which becomes one more source; monitoring | engine | **Landed**: JACK 2026-09-19 with monitoring and the input meter, Core Audio 2026-09-20 -- see below |
| [06](06-unused-takes.md) | Find and delete takes nothing refers to | session, project, UI build | **landed**: quit prompt 2026-09-20, the clean-up command and dialog 2026-09-23 (MOO-38) -- see below |

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

## What step 05 actually did

- **The AUDIO row** in the channel sidebar (`channel-sidebar.slint`), built
  by `AudioInputPicker` and offered on every channel.
- **The RECORD page** on the sampler face: REC (idle / WAIT / STOP), the
  elapsed length, the take's waveform from the drain's peaks, CLIP and LENGTH
  (1-64 bars, saved as `SamplerState::record`), and FROM. The pump publishes
  the live half each tick; `Session::record_press` decides what REC does and
  is unit-tested; pressing it on a stopped transport sends Play first.
- **The acceptance, run in the real application**, headless with the engine
  and JACK, through `MOOLOOP_AUTODRIVE_RECORD=1` (`OPERATIONS.md`) --
  because the AUDIO row's menu is a popup the MCP tools cannot click. It
  passes: the page showed the pre-roll and the growing take, the take is one
  bar (96,000 frames, sound in every tenth of it), it became the sampler's
  sample, owned by the song, as a "Record Take" undo step. Screenshots taken
  over MCP during the run show WAIT, then STOP at "0.6 / 1 BARS" with the
  waveform filling toward the clip length. Running it found one bug: CLIP read
  as off when the page was reopened, because only a click wrote the window's
  copy; the callback writes it back now.
- **Covered elsewhere, not re-run here:** save and reload of a take
  (`a_take_is_copied_into_the_song_in_either_mode`), two samplers at once
  (`two_channels_take_at_once`), an offline render of a sampler with a
  sample (the ordinary path). Undo was checked as the history entry, not by
  pressing it.
- **Not heard.** Nobody has listened to a take yet. The listening pass
  owed for recording and undo together (MOO-165) was closed on 2026-09-22 on
  Adam's instruction to treat outstanding listening passes as done with
  nothing heard.

## What step 01 actually did (so far)

- **The input bus.** `Executor::process_with_input` copies the driver's input
  into a preallocated `StereoBus` on the renderer before anything renders;
  `process` passes none, so offline renders and every existing caller are
  unchanged and silent on it. Tested for content and for no allocation.
- **JACK:** `mooloop:in_l` and `in_r`, auto-connected to the first two
  physical capture ports as `midi_in` is to the MIDI sources. The AUDIO menu
  lists one input, "Audio In", for the reason the MIDI menu lists one merged
  input: what feeds it is chosen in the JACK graph.
- **`AudioInputSource::Input` is one value, not `Port(String)`** as the step
  asked. There is one input pair, and a name would have cost `Copy` on a type
  copied everywhere for nothing it could yet distinguish. When a driver offers
  several, it grows the name.
- **Latency.** A take from the input starts the JACK round trip (the output's
  playback latency plus the input's capture latency) after its bar line, and
  the clip length counts from there, so the take lines up with what the
  performer heard. `a_take_from_the_input_records_the_input_after_its_latency`
  feeds a frame counter and checks the first frame is the bar plus the delay.
- **Run live under JACK (PipeWire), 2026-09-19**, with
  `MOOLOOP_AUTODRIVE_RECORD=input`: `in_l`/`in_r` connected to the laptop's
  digital microphone, capture latency 1,056 frames and playback 1,048. The
  microphone was muted in the OS, so that take was silent, correctly; a 440 Hz
  tone played by `pw-cat` and connected to `mooloop:in_l`/`in_r` arrived at
  exactly its generated peak (0.2441). Patching mooloop's own output into its
  input records silence -- PipeWire breaks the loop -- which is not a fault.
- **Monitoring and the input meter**, landed the same day: a MON toggle and a
  peak meter under the AUDIO row, shown when the hardware input is picked.
  Monitoring is per channel, kept by `ChannelId` in the session, never saved,
  off by default and never switched on by anything but the toggle; it reaches
  the engine as `EngineCommand::SetInputMonitor` and rides `InputState`
  across an install. A monitored channel adds the input bus before its
  devices, fader and pan, and **never sleeps** -- the one finding here:
  `monitoring_plays_the_input_through_its_channel_and_nothing_else_does` first
  passed without that guard, because a sounding input keeps its own strip
  awake; the version that starts the input after two seconds of silence fails
  without it.
- **Core Audio's input stream, 2026-09-20**, written and checked on the Mac.
  cpal has no duplex stream, so it is a second stream on the system's default
  input device, on its own thread and its own device clock, handing stereo
  pairs to the output callback through an `rtrb` ring. `audio_input_label`
  returns that device's own name -- Core Audio opens a device rather than
  joining a graph, so unlike JACK there is a device to name -- and returns
  `None`, which is no input row at all, when there is no input device, when
  the device will not run at the engine's sample rate, or when macOS has not
  granted the microphone. That last one is logged as a sentence naming
  Privacy & Security, because macOS refuses silently and a stream that never
  delivers reads as a broken driver.

  Four things the doing decided, none of them a design question the step
  asked:

  - **The prefill is two buffers, not the half-ring the step specified.** A
    prefill is input latency the performer hears, and half of an eight-buffer
    ring is 85 ms at 1024 frames. Two buffers still leaves six of headroom,
    which is what the step was buying.
  - **Drift is counted, not corrected**, as the step asked. Frames read as
    silence and frames dropped are counted, and `service` reports the total
    when it moves -- once a second at worst, and never on a machine whose
    input and output are one device, which has no drift to report. There is no
    resampler and a long take from a second device will slip.
  - **One input device, the system's**, not a picked one. An input target
    would be a second `<device>#<channel>` pair through the settings file and
    the preferences page, and nothing has asked for one.
  - **`input_latency_frames` is an estimate here, where JACK's is a
    measurement.** JACK asks the server for capture and playback latency;
    cpal reports neither. What is returned is the path through this driver --
    a buffer out, the prefill, a buffer in -- so a take is right to within the
    device's converter delay rather than to the sample.

  **Run live on the Mac, 2026-09-20**, which is step 01's own acceptance and
  the JACK half's twin: `MOOLOOP_AUTODRIVE_RECORD=input` with a throwaway
  `MOOLOOP_CONFIG_DIR`. It opened the machine's default input -- a webcam's
  microphone, `IC800 1080P HD` -- at 48 kHz, showed the pre-roll and the
  growing take, and the report was PASS: the take became the sampler's
  sample, owned by the song, as one "Record Take" undo step.

  **And the file was opened rather than trusted**, because the report says a
  take arrived and not that it has anything in it -- the JACK run on
  2026-09-19 recorded correct silence from a muted microphone and passed the
  same checks. It is 96,000 frames, one bar at 120 bpm at 48 kHz, to the
  sample; it peaks at -9.9 dBFS and averages -25.3 dBFS; 99.9% of its samples
  are non-zero and every tenth of it has sound in it. So the ring really is
  carrying the device's audio, not zeros.

  Worth noting for the drift limitation: the input in that run was a webcam
  and the output was not, so the two clocks really were different clocks, and
  **nothing was reported over the whole run**. That is one short take, not
  evidence about a long one.

  The unit tests that landed with it cover the ring (`InputTap::fill` and the
  interleaved copy), each validated by mutation against the tree before its
  own fix.

  It also retired a `LOOSE_ENDS.md` entry on the way past: the Core MIDI port
  ids, written on Linux on 2026-09-15 and never compiled, compile and their
  tests run.

- **`Executor::process` is now test-only.** Core Audio was the last caller
  that had no input; both drivers go through `process_with_input`, and its
  `dead_code` exemption says so rather than naming a driver that has moved
  on.

## Open questions for Adam

Each of these changes what a step builds. They are listed here so the step
that needs an answer can stop and ask rather than guess. All five were
answered the day the plan was written, and are kept here as a record.

**This list and "Adam's decisions" above are numbered separately, and they now
collide.** Decision 9 is the pre-roll and decision 10 is the AUDIO row; open
question 9 is quitting mid-take and open question 10 is arming on a missing
input. A bare "decision 9" in a comment is therefore ambiguous, so code citing
either says which list it means. Found 2026-09-21, when both lists reached ten.

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

9. **Quitting with a take still being written.** Adam's call, 2026-09-21:
   **finish it, then quit.** Quit ends the take the way pressing Stop does,
   flushes the ring and finalizes the WAV, and only then leaves -- no prompt,
   because what is outstanding is a fraction of a second rather than
   something worth a dialog. The wait is **bounded** so a stuck drain cannot
   hang quit, and a wait that times out removes the partial file rather than
   leaving an unfinalized one in `recordings/`. Note what this does *not*
   promise: the finished take is a complete file on disk, not a sample in the
   song, because the song is closing. **Built the same day** as
   `TakeRecorder::finish_all` with a `Drop` behind it; see *a take is owned
   at all four edges* below. Step 04.

10. **Arming record on a channel whose input has gone.** Adam's call,
    2026-09-21: **refuse, and name the cause.** The two causes need different
    fixes from the user -- a resampled channel that was deleted, versus no
    audio input device at all (unplugged, changed, or a refused microphone
    permission on macOS) -- so one generic message sends them looking in the
    wrong place. What this closes is silent and destructive: arming on a
    missing input records digital silence and the finished take then
    *replaces the channel's sample*. **Built the same day** as
    `RecordPress::SourceGone` and `RecordPress::NoInputDevice`, asking the
    picker rather than `AudioInputSource::resolve`; see *a take is owned at
    all four edges* below. Step 04.

## Closed: a take is owned at all four edges

**Reported twice, fixed 2026-09-21.** Two consecutive review runs
(`reports/fable-2026-09-20.md` finding 2, `reports/fable-2026-09-21.md`
finding 3) reported the same four edges, and neither this file nor
`LOOSE_ENDS.md` had recorded them -- so the second run had to rediscover what
the first had found. All four are now closed by MOO-55:

- **Quit no longer ends the process with the WAV header unpatched**, and
  does it **without asking** (open question 9). `TakeRecorder::finish_all`
  ends every live take and joins every drain against a bounded deadline;
  `AppUi::finish_takes` runs it once the event loop returns, and `impl Drop
  for TakeRecorder` backs it up for the routes that never reach there. A wait
  that times out removes the partial file rather than leaving an unfinalized
  one. `TakeStatus::end` is the one control-side phase write, needed because
  at quit the engine may already be going away and the drain's exit condition
  could otherwise never be met.
- **A failed drain removes its partial file**, from both the write and the
  finalize route, through one `failed()` helper that also says so in the
  message.
- **A take lands only on a channel that is still a sampler**, via
  `Session::take_target` and `TakeMiss`.
- **REC refuses a source that has gone, and names which kind** (open question
  10): `RecordPress::SourceGone` for a deleted channel or track,
  `RecordPress::NoInputDevice` for no hardware input at all. One message would
  send half of them looking in the wrong place. The rule is the picker's --
  the one the AUDIO row already draws -- and deliberately not
  `AudioInputSource::resolve`'s, which calls the hardware input present
  whatever the driver offers.

**Two of these were built before Adam answered, and then changed to match.**
The first pass gave quit a prompt and used one generic `SourceMissing`; open
questions 9 and 10 came back "no prompt" and "a variant per cause", so both
were rebuilt. Worth recording because the first pass was not wrong to guess --
it was wrong to ship the guess without marking it as one.

**The lesson is about the gap between Linear and the tree.** This entry
previously read "MOO-55 is marked Done without a fix commit existing", and it
was right about what it could see: the fix was committed locally and had not
been pushed, so a reviewer checking `git log` and grepping for `finish_all`
found nothing and correctly concluded the issue had been closed on nothing.
Two of the four were then fixed a second time, independently. **An issue is
not done until its fix is on `origin`**; closing it earlier costs somebody
else the same work twice.

This is step 04's contract rather than step 06's, so it is written here and
not in `06-unused-takes.md`.

## What step 06 actually did

**The quit prompt landed 2026-09-20 and the clean-up dialog 2026-09-23**
(MOO-38). The prompt went first on Adam's call, as the half the step file
itself says "handles the common case without ever opening the dialog".

What is in: `mooloop-session/src/recordings.rs` answers which takes nothing
refers to -- the open session's channels *and* every undo and redo snapshot,
so a take an undo could reach back to is never offered -- and hands them to a
`Trash` trait, whose real implementation is the desktop's own trash via the
`trash` crate. Both quit paths ask, after the decision to quit is settled and
never before it, so a cancelled quit tidies nothing away.

**Deliberately outside the prompt: anything older than this run.** Only a
crash leaves a shared-folder take behind, and it may be the only copy of a
song that was never saved. A yes/no prompt is the wrong instrument for that
question, so `UnusedTake::from_earlier_session` marks them and the quit path
filters them out. Listing them, unticked and with the reason stated, is the
dialog's job.

**The project-assets half stays in, and a song has a `recordings/` folder**
(Adam, 2026-09-22, MOO-38 question 1: *"having a recordings/ doesnt sound like
a terrible idea. that rebuild is a problem, too. it keeps rewriting the
filenames every save."*). Landed the same day in `mooloop-project`: a save
copies a take into `<song>.mooloop-assets/recordings/` under the name it was
recorded with, and **the sidecar is added to rather than rebuilt** -- a file
already in it keeps its place and name and is not copied again, and a file the
song stops using stays there for the clean-up dialog. That also closes the
sequence MOO-89 suspected (a save deleting the copy of a take the undo history
still points at): `an_earlier_take_survives_the_save_after_the_retake_that_replaced_it`.
`PROJECT_FORMAT.md` has the folder and the rule.

**The dialog, 2026-09-23.** `recording.clean-up` (File > Clean Up Takes, in
`ACTIONS.md`) opens `TakesDialog` with the plan's two lists, built by
`recordings::clean_up`:

- *Not used by this song*, ticked: this session's shared-folder takes that
  nothing reaches, and takes in the song's own `recordings/` that neither the
  live song, its history **nor the song as saved on disk** reaches
  (`saved_song_references`, the third reference source). A song never saved,
  or one whose file will not read, offers nothing from its folder.
- *Left from earlier sessions*, unticked, with the reason stated
  (`EARLIER_SESSIONS_NOTE`).

Each row has its name, length (from the WAV header), size and date (in UTC,
labelled so: the take's own file name is UTC and the crate carries no
time-zone database), and the button says the running total it will move.
**The quit prompt is the same dialog now**, opened with "Trash and Quit" and
"Keep and Quit", because MOO-91 took every question off `confirm_dialog`: a
zenity question that would not start read as No, and one that did blocked the
UI thread.

**The quit scan still counts the undo history and the unsaved song** (MOO-38
question 2). Adam, 2026-09-22: *"i dont know. i dont really have enough
context to understand the question. this seems like it is regarding only an
edge case?"* That is not a ruling, so the option that loses nothing was
taken: at quit, a take the history or the unsaved song still reaches is not
offered. The cost is the edge case he named -- a retake that replaced an
earlier take keeps the earlier one "referenced" through its Record Take entry,
so the quit does not offer it -- and it is not lost track of: by the next run
it is an earlier-session file, listed by the dialog, unticked, with the reason
stated. Ignoring the history at quit would have offered, ticked, a take whose
only reference is a song the user chose not to save; the trash would make
that recoverable, but not obviously so.

## Not in this plan

- Per-track audio clips and anything else DAW-like (decision 1).
- ~~Recording more than one channel at a time.~~ Withdrawn 2026-09-18
  (decision 6): several channels may record at once.
- Separate input ports per MIDI device under JACK (`CONTROL_SURFACES.md`).
- `#7`, the full `AudioBackend` boundary. Step 01 explains why it is not a
  prerequisite, and what would change that.
