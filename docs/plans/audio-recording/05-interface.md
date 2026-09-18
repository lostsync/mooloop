# 05 — The interface

## Build

**One `main.slint` contract change, shared with step 02.** Draft everything
in `scripts/slint-sketch` first.

- **Record button.** The tooltip currently says MIDI. Tooltips here state a
  value only, so it becomes "Record", and the status bar explains what will be
  captured: "Records <source> into <channel>'s sampler", where the source is
  a track, a channel, the master or a hardware input, or "Records notes from
  <input>".
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
- **The non-sampler rule** from step 02, made visible: greyed-out audio rows,
  with the reason in the status bar.

## Acceptance

**Resample a loop.** A drum track plays a pattern. Pick that track as a
sampler channel's source, arm, record two passes, stop. Then:
- The sampler holds the take, and a pattern that triggers it plays it back.
- Undo puts back the previous sample.
- It survives save and reload.
- It renders offline the same way it plays.
- Resampling the sampler channel into itself, through a Buffer or a reverb,
  replaces its sample with the processed take.

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
