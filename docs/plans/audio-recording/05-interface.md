# 05 — The interface

## Needs an answer first

Open question 4 in `00-status.md`: monitoring.

## Build

**One `main.slint` contract change, shared with step 02.** Draft everything
in `scripts/slint-sketch` first.

- **Record button.** The tooltip currently says MIDI. Tooltips here state a
  value only, so it becomes "Record", and the status bar explains what will be
  captured: "Records audio from <input> into <channel>'s sampler" or
  "Records notes from <input>".
- **Input meter.** Show one on the channel's IN row, or beside the record
  button, whenever an audio input is selected, whether or not the channel is
  armed. You need to see a level before you record.
- **While recording.** The sampler face shows the waveform growing, drawn
  from the drain thread's peak summary, and the channel row shows that it is
  recording.
- **A damaged take** (overflow, from step 03) says so in the status bar and in
  the sample's description.
- **Monitoring is a toggle** (Adam, 2026-09-17). When it is on, a channel
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

Record a phrase from a microphone or a line input into a sampler channel
while the pattern loops, stop, and play it back.
- A pattern that already triggers the sampler plays the take.
- It survives save and reload.
- It renders offline the same way it plays.

Run it through the live app (`scripts/mooloop-mcp`) and listen.

## Docs

- `CURRENT.md`: the new surface.
- `SCOPE.md` §2 items 3 and 5: done.
