# Current System

Status: implementation snapshot, August 2026.

This document describes the prototype as implemented. It is deliberately
blunt about gaps so roadmap decisions are based on the system that exists.

## Implemented User Surface

- One application window with a two-row toolbar, channel rack, and a lower
  editor. The transport row carries play/stop, pattern-vs-song mode, a
  bar:beat:tick position readout, beat lamps, drag-or-type tempo, and the master
  meter; the edit row carries pattern selection, the cursor tools, pattern
  length, and grid snap.
- Patterns are chosen with a fixed-width stepper plus a jump menu and can be
  named; the selector costs the same width at any pattern count.
- Four cursor tools drive the rack grid: Select (click toggles, ctrl-drag sets
  velocity), Paint (drag fills, right-drag clears), Slice (ratchet a step into
  2-4 even hits), and Stretch (drag a step sideways to set note length). The
  whole run of steps shares one hit area, because a per-cell one cannot follow
  a drag past the cell the press landed in.
- Up to 16 channels. The UI starts with one sampler channel and can append or
  remove channels. Every rack row exposes mute, output volume, and
  constant-power stereo pan.
- Patterns are created explicitly from a one-pattern project, with up to 256
  addressable pattern IDs and independent logical lengths from 1 to 256 steps.
  Hidden steps survive shortening and re-extending a pattern.
- Pattern and Song transport modes are independent of the visible editor.
  The playlist is a lower-pane tab, supports layered tick-addressed pattern
  instances, and remains editable while either mode plays. Clip width follows
  each pattern's natural length.
- Tick-addressed notes with stable IDs, start, duration, MIDI pitch, and
  velocity. Starts snap to 64ths in the piano roll while retaining PPQ tick
  precision internally.
- A horizontally and vertically zoomable piano roll with note creation,
  movement, length resizing, right-click removal, exact pitch/velocity/length
  fields, and a pinned velocity lane. It shares selectable straight/triplet
  musical snap values from one bar through 1/64 with the playlist.
- Sixteenth-note rack cells summarize their four 64th-note substeps without
  discarding rests between hits. Each substep is drawn solid where a note is
  struck and dim where one is merely held, so a ratcheted step is
  distinguishable from a single sustained note; coverage alone renders both as
  a full cell.
- A sampler editor with waveform, WAV loading and sibling navigation, trim,
  reverse, root note, coarse/fine tune, loop region and mode, ADSR, low-pass
  filter with envelope depth and resonance, drive, bit reduction, and rate
  reduction. Voice controls cover one-shot/gated playback, 1-16 voices,
  restart/layer retriggering, and 16 cross-channel choke groups.
- Runtime appearance presets, custom accent persistence, shared audio controls,
  tooltips, and master peak-meter ballistics.
- A File menu for song, kit, and selected-channel save/load. Song documents use
  versioned directory bundles with an inspectable TOML manifest and optional
  copied WAV assets. Missing or corrupt samples warn and load as silent slots.
- Offline export of exactly one selected-pattern pass in Pattern mode or one
  derived playlist pass in Song mode, followed by a configurable 0-30 second
  release tail. Outputs are 24-bit PCM WAV, 32-bit float WAV, or 192/256/320
  kbps MP3.
- A shared widget library in `crates/mooloop-ui/ui`: knobs with value arcs and a
  bipolar mode (`controls.slint`), LED-segment metering with scales, latching
  clip indicators, gain-reduction and correlation meters (`meters.slint`), and a
  draggable graphical ADSR (`envelope.slint`). `cargo run -p mooloop-ui --example
  control-gallery` shows every control; set `MOOLOOP_GALLERY_SNAPSHOT` and
  `MOOLOOP_GALLERY_SIZE` to capture it headlessly.
- Some widgets exist ahead of the features that will use them: gain reduction and
  correlation have no audio behind them yet, and solo is a button style only.
- There is no metronome. The toolbar deliberately does not offer a click-track
  toggle, since nothing in the DSP graph produces one yet.

## Current Audio Path

```text
UI commands -> rtrb queue -> shared render state -> transport + sequencer
                                  |
                                  v
                         timed events/channel
                                  |
                                  v
sample slot -> bounded sampler voices -> empty effect vector -> gain/pan/mute
                                                           |
                                                           v
                                                    master stereo bus
                                                           |
                                                           v
                                                       JACK outputs
```

The engine preallocates channel strips, pattern storage, event lists, and audio
buses. A JACK-independent render state owns transport, scheduling, instruments,
effects, mixing, and metering. The JACK adapter drains fixed-size commands into
that state and publishes position and master peak events; offline export drives
the same render path without JACK ports.

WAV decode, waveform construction, and directory scanning occur off the audio
thread. A decoded sample is published through an `ArcSwapOption` slot.

## Useful Foundations

- `AudioNode` provides one in-place DSP interface for instruments and effects.
- `DrumSynth` (kick/snare/hat) and `MonoSynth` (three-oscillator, mono, glide)
  implement that interface with parameters in `mooloop-core`, but are not yet
  wired into channels, the bridge, or the UI.
- `EventList` carries fixed-capacity, sample-timed NoteOn, NoteOff, and generic
  ParamValue events.
- `StereoBus` ownership is centralized in the graph, leaving room for sends,
  groups, sidechains, and buffer taps.
- Musical time is PPQ 96, which exactly represents common subdivisions through
  64th notes and triplet grids.
- Pattern and channel capacity are bounded for realtime safety.
- DSP tests cover sampler pitch, trim, loops, envelopes, filter behavior,
  reverse playback, and lo-fi stages, plus drum synth and mono synth voice,
  envelope, glide, and filter behavior.

## Important Limitations

### Event And Voice Model

- Probability, microtiming controls, ties, and parameter locks are not yet
  implemented. Note starts and lengths otherwise retain PPQ precision.
- NoteOn, NoteOff, and choke events are sample-accurate and deterministically
  ordered. One-shot loops exit into their remaining sample tail; gated loops
  release through the amplitude envelope.
- Sampler voice allocation is fixed-capacity and deterministic: restart reuses
  the oldest matching pitch, layer mode overlaps notes, and overflow steals
  the oldest voice.

### Transport And Arrangement

- Pattern mode loops the selected pattern. Song mode layers playlist placements
  on the shared absolute clock and loops at the bar after the furthest clip end.
- Playlist starts use the shared musical snap while retaining absolute PPQ
  ticks and are bounded to a 64-bar start canvas. The timeline is horizontally
  zoomable. There is no clip dragging, explicit song loop range, time-signature
  model, swing, groove, or per-channel timing offset.

### State And Persistence

- `mooloop_core::Project` is the canonical serializable snapshot shared by the
  UI, realtime engine installation, persistence, and offline renderer. The
  live UI still owns incremental edits and produces snapshots for these paths.
- Songs, kits, and channel presets use the v1 bundle contract documented in
  `PROJECT_FORMAT.md`. Saves stage and replace bundles atomically; embedded and
  referenced asset policies are available per save.
- Missing samples are recoverable by loading a replacement WAV, but there is no
  dedicated path-search/relink dialog, undo, autosave, or crash recovery yet.

### Mixing, Routing, And Effects

- Channel mute, volume, and pan are exposed, as compact knobs in the rack row.
  Metering is master-only; per-channel meters are drawn but unfed.
- The channel effect vector is empty and there is no realtime-safe graph edit
  protocol for inserting or reordering devices.
- There are no sends, returns, groups, sidechains, external inputs, or routing
  controls.

### Buffers And Rendering

- Samples are immutable loaded assets. Channels do not yet record their own
  output into working audio memory.
- The render graph is independent of JACK and supports finite offline passes.
  WAV uses the active JACK sample rate; MP3 renders at 48 kHz through an
  in-process LAME encoder. Stem/bus export and realtime-vs-offline null testing
  are not implemented.
- Replaced sample lifetimes need a deliberate deferred-reclamation design so
  the last large sample allocation can never be freed on the realtime thread.

### Interface

- The application is usable but still has interaction and responsive-layout
  edge cases.
- The lower parameter selector only implements Velocity.
- There is no canonical command/shortcut layer, selection model, undo stack,
  or project-level navigation.

## Architecture Risks To Resolve Early

1. Add probability and explicit microtiming controls without weakening the
   tick-addressed event contract before broad automation.
2. Define fixed-capacity device and routing edits before populating the empty
   effects vector.
3. Budget channel buffer memory and specify read/write collision behavior
   before buffers become part of every strip.
4. Add deferred reclamation for replaced samples, graphs, and future buffers.

## Verification Commands

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p mooloop-app --bin engine-selftest
MOOLOOP_AUTODRIVE=1 cargo run -p mooloop-app --bin mooloop
```
