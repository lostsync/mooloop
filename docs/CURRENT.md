# Current System

Status: implementation snapshot, August 2026.

This document describes the prototype as implemented. It is deliberately
blunt about gaps so roadmap decisions are based on the system that exists.

## Implemented User Surface

- One application window with transport, eight pattern selectors, tempo,
  master peak meters, channel rack, and a lower editor.
- Up to 16 channels. The UI starts with one sampler channel and can append or
  remove channels.
- Eight patterns with independent logical lengths from 1 to 256 steps. Hidden
  steps survive shortening and re-extending a pattern.
- Tick-addressed notes with stable IDs, start, duration, MIDI pitch, and
  velocity. Starts snap to 64ths in the piano roll while retaining PPQ tick
  precision internally.
- A horizontally and vertically zoomable piano roll with note creation,
  movement, length resizing, right-click removal, exact pitch/velocity/length
  fields, and a pinned velocity lane.
- Sixteenth-note rack cells summarize their four 64th-note substeps without
  discarding rests between hits.
- A sampler editor with waveform, WAV loading and sibling navigation, trim,
  reverse, root note, coarse/fine tune, loop region and mode, ADSR, low-pass
  filter with envelope depth and resonance, drive, bit reduction, and rate
  reduction.
- Runtime appearance presets, custom accent persistence, shared audio controls,
  tooltips, and master peak-meter ballistics.

## Current Audio Path

```text
UI commands -> rtrb queue -> transport + sequencer
                                  |
                                  v
                         timed events/channel
                                  |
                                  v
sample slot -> monophonic sampler -> empty effect vector -> gain/pan/mute
                                                           |
                                                           v
                                                    master stereo bus
                                                           |
                                                           v
                                                       JACK outputs
```

The engine preallocates channel strips, pattern storage, event lists, and audio
buses. The JACK callback drains fixed-size commands, advances transport,
schedules events at sample offsets, renders each active strip, and publishes
position and master peak events.

WAV decode, waveform construction, and directory scanning occur off the audio
thread. A decoded sample is published through an `ArcSwapOption` slot.

## Useful Foundations

- `AudioNode` provides one in-place DSP interface for instruments and effects.
- `EventList` carries fixed-capacity, sample-timed NoteOn, NoteOff, and generic
  ParamValue events.
- `StereoBus` ownership is centralized in the graph, leaving room for sends,
  groups, sidechains, and buffer taps.
- Musical time is PPQ 96, which exactly represents common subdivisions through
  64th notes and triplet grids.
- Pattern and channel capacity are bounded for realtime safety.
- DSP tests cover sampler pitch, trim, loops, envelopes, filter behavior,
  reverse playback, and lo-fi stages.

## Important Limitations

### Event And Voice Model

- Probability, microtiming controls, ties, and parameter locks are not yet
  implemented. Note starts and lengths otherwise retain PPQ precision.
- NoteOn and NoteOff events are sample-accurate and deterministically ordered,
  but the sampler has not yet gained the full Phase 2 voice-mode surface.
- The sampler has one voice. A new note cuts and retriggers the previous one.
- There are no choke groups, voice modes, or explicit one-shot versus gated
  behavior.

### Transport And Arrangement

- There is only pattern transport. The absolute clock advances continuously
  and the sequencer wraps step lookup modulo the current pattern length.
- There is no playlist, Song mode, song loop, time-signature model, swing,
  groove, or per-channel timing offset.

### State And Persistence

- The UI-side `UiState` is the editable source of truth and mirrors mutations
  to the engine. It is not a durable project model.
- There is no project, kit, undo, autosave, recovery, or missing-sample relink
  format.
- Several useful core model types are not yet the canonical application state.

### Mixing, Routing, And Effects

- Channel gain and pan exist internally but have no complete command/UI path.
- Only channel mute is exposed. Metering is master-only.
- The channel effect vector is empty and there is no realtime-safe graph edit
  protocol for inserting or reordering devices.
- There are no sends, returns, groups, sidechains, external inputs, or routing
  controls.

### Buffers And Rendering

- Samples are immutable loaded assets. Channels do not yet record their own
  output into working audio memory.
- The realtime graph is coupled to JACK's process callback and ports. There is
  no offline render driver or WAV/MP3 export.
- Replaced sample lifetimes need a deliberate deferred-reclamation design so
  the last large sample allocation can never be freed on the realtime thread.

### Interface

- The application is usable but still has interaction and responsive-layout
  edge cases.
- The lower parameter selector only implements Velocity.
- There is no canonical command/shortcut layer, selection model, undo stack,
  or project-level navigation.

## Architecture Risks To Resolve Early

1. Define a canonical `Project` model before playlist, undo, persistence, and
   rendering create four competing state representations.
2. Complete sampler polyphony and voice modes on top of the tick-addressed
   event contract before adding synth polyphony or broad automation.
3. Separate the render graph from the JACK adapter so realtime and offline
   rendering execute the same DSP path.
4. Define fixed-capacity device and routing edits before populating the empty
   effects vector.
5. Budget channel buffer memory and specify read/write collision behavior
   before buffers become part of every strip.
6. Add deferred reclamation for replaced samples, graphs, and future buffers.

## Verification Commands

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p mooloop-app --bin engine-selftest
MOOLOOP_AUTODRIVE=1 cargo run -p mooloop-app --bin mooloop
```
