# 05 — The interface

> **Rewritten 2026-09-18** around decisions 7-10: recording is started from
> the sampler's face, not the global record button, and the audio input is
> its own sidebar row.

## Build

**One `main.slint` contract change, shared with step 02.** Draft everything
in `scripts/slint-sketch` first; the face is sketched at its real size
before anything else, as every face is.

- **The AUDIO row** in the channel sidebar, beside MIDI IN and independent of
  it: Off, the master, every track, every channel. Offered on every channel,
  whatever its device (decision 5) -- a device that has no use for it ignores
  it. A deleted source shows as missing, as an unplugged MIDI port does.
- **A Record page on the sampler face.** Its own record button; a
  live-updating waveform of the take as it grows, drawn like Buffer's history
  from the drain thread's peak summary; the state -- idle, waiting for the
  bar (the pre-roll), recording, with the elapsed length in bars; and the
  clip controls: a **Clip** toggle and a **Length** on the shared
  `ModTimeDivision`/bars grid, which ends the take by itself when on and is
  ignored when off. The global record button stays MIDI's; its tooltip is
  "Record" and nothing about it changes here.
- **A take on a channel with no audio input** is refused at the button,
  with the reason in the status bar.
- **Source meter.** Show one on the channel's IN row, or beside the record
  button, whenever an audio source is selected, whether or not the channel is
  armed. You need to see a level before you record. For an app source this
  is the meter that source already has, so this step shows it here rather
  than measuring anything new. A hardware input's meter arrives with step 01.
- **While recording.** The sampler face shows the waveform growing, drawn
  from the drain thread's peak summary, and the channel row shows that it is
  recording.
- **A damaged take** (overflow, from step 03) says so in the status bar and in
  the sample's description.
- **Monitoring is a toggle** (Adam, 2026-09-17), **and it arrives with step
  01**: it exists only for a hardware input, because an app source is
  already audible (open question 8). The notes below are kept here because
  they are about the IN row. When it is on, a channel
  with an audio input plays its input bus where its source would play. That
  is `AuxIn`'s level-and-copy applied to the input bus, which is why this is
  an interface step and not an engine one.
  - **Placement:** sketch per channel, next to the IN row. That is where the
    input is chosen, so that is where you would look for whether you can hear
    it.
  - **Not saved in the project.** It is performance state, like record arm.
    It travels to the engine in `InputState`, so a project install doesn't
    silently switch it off, which is the bug record arm had until
    2026-09-17.
  - **Feedback:** monitoring through speakers with a live microphone
    feeds back. Default the toggle to off, and never turn it on
    automatically.

## Acceptance

**Resample a loop.** A drum track plays a pattern. Pick that track as a
sampler channel's AUDIO input, press record on its Record page with Clip on
and a length of 2 bars, and let it end by itself. Then:
- The sampler holds the take, and a pattern that triggers it plays it back.
- Undo puts back the previous sample.
- It survives save and reload.
- It renders offline the same way it plays.
- Resampling the sampler channel into itself, through a Buffer or a reverb,
  replaces its sample with the processed take.
- Two sampler channels, each with its own AUDIO input, recording at once,
  each get their own take.

Run it through the live app (`scripts/mooloop-mcp`) and listen. This is where
the plan first produces something you can play with, and it involves no
driver.

**Record from a microphone** is the same test with a hardware input as the
source. It is step 01's acceptance, because that is where the input
appears:
- A pattern that already triggers the sampler plays the take.
- It survives save and reload.
- It renders offline the same way it plays.

Run it through the live app (`scripts/mooloop-mcp`) and listen.

## Docs

- `CURRENT.md`: the new surface.
- `SCOPE.md` §2 items 3 and 5: done.
