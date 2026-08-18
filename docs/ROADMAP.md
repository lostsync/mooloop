# Roadmap

Status: dependency-ordered working plan, August 2026.

The old labels treated effects as Phase 3 and synths as Phase 4. That order no
longer reflects the product. Effects depend on a stable parameter event model,
and synth design may depend on the channel buffer. This roadmap orders work by
those dependencies rather than by visible feature category.

## Phase 1: Finish The Working Instrument

Goal: make the current sampler/rack/piano-roll prototype dependable enough to
use while deeper models change underneath it.

Scope:

- Resolve remaining single-click, focus, selection, zoom, scrolling, and
  narrow-window issues.
- Establish consistent right-click removal and keyboard navigation.
- Finish sampler voice controls that do not require the new event model.
- Add per-channel meters through the existing strip; volume and pan are exposed.
- Keep the UI control gallery as the interaction contract.

Exit criteria:

- Common controls respond on the first action.
- The selected channel, step, note, and editor target are always visible.
- The sampler can be used comfortably without opening a separate DAW.
- Existing tests, strict Clippy, autodrive, and tiled-window visual checks pass.

## Phase 2: Voice Behavior And Expressive Events

Goal: replace the fixed one-slot step model with a musical event model capable
of serious sequencing.

Scope:

- Tick-addressed notes with start, duration, pitch, velocity, and stable ID.
- At least 64th-note entry and movement; retain PPQ tick precision internally.
- NoteOff scheduling and defined event ordering at equal timestamps.
- Sampler one-shot and gated modes, polyphony, voice stealing, choke groups,
  retrigger behavior, and loop release behavior.
- Probability, microtiming, and per-channel timing offset after deterministic
  placement works.
- Rack cells summarize substeps while the piano roll edits full events.
- Parameter lanes edit selected note data without inventing separate state.

Exit criteria:

- Two or four events can occupy one sixteenth-note rack cell and are rendered
  at the correct sample offsets.
- Note duration audibly controls gated and looped voices.
- Voice behavior is covered by deterministic DSP and scheduler tests.

## Phase 3: Project, Patterns, And Song

Goal: make work durable and arrange variable-length patterns coherently.

Scope:

- A canonical, versioned `Project` model shared by UI, persistence, compiler,
  and renderer.
- Save/load, autosave or recovery, missing-sample handling, and undoable edits.
- Pattern duplicate, rename, copy/paste, and independent lengths.
- Layered playlist placements on a shared tick or bar timeline.
- Pattern and Song transport modes with independent loop ranges.
- Swing and groove features built on explicit timing displacement.
- Extract the render graph from JACK and render offline to WAV.

Different pattern lengths:

A placement has a start time and references a pattern. Its default duration is
that pattern's natural tick length. Placements may cross bars and overlap.
The song loop is a separate range and never changes a pattern's own length.

Exit criteria:

- A saved project reopens with identical patterns, samples, and arrangement.
- Mixed-length patterns layer and loop predictably in Song mode.
- Offline WAV output matches realtime playback within defined tolerances.

## Phase 4: Channel Buffer Spike

Goal: decide whether persistent channel audio memory is the reason for mooloop
to exist.

Implement only the bounded spike in `BUFFER_ENGINE.md`. Do not build a general
looper, destructive audio editor, or large synth suite during this phase.

Exit criteria:

- A running channel continuously retains a bounded span of source audio without
  a record gesture or transport stop.
- A following read head behaves like a trustworthy live bridge, then can jump,
  loop, change rate, reverse, and return live under sequencer control.
- Head and history state are understandable, resettable, persistable when
  musically required, and realtime safe.
- The workflow produces something materially faster or different than manual
  bounce-to-sample. If it does not, revise or reject the thesis.

## Phase 5: Automation And Effects

Goal: make sound shaping composable with notes, buffers, and arrangement.

Scope:

- Stable parameter IDs and metadata: range, scale, units, default, polarity.
- Sample-accurate ParamValue scheduling with smoothing where required.
- The lower lane selects note, channel, buffer, and device targets through one
  target browser.
- Realtime-safe fixed-capacity insert editing, bypass, ordering, and state.
- A small authored initial effect set: filter, delay, saturation/color, and
  utility dynamics or EQ only where the signal flow needs them.
- Buffer placement explicitly accounts for pre/post insert signal flow and any
  future feedback path.

Exit criteria:

- The same lane interaction automates sampler, buffer, mixer, and effect
  parameters.
- Device edits cannot allocate, lock, or free large objects on the audio
  thread.

## Phase 6: Synth Sources And Routing

Goal: add generated sound that exploits the established voice and buffer model.

Scope:

- A percussive synth capable of kicks, snares, toms, hats, and synthetic noise
  material.
- A compact tonal or wavetable/buffer-derived source chosen for glitch and
  industrial use rather than workstation coverage.
- Source-to-buffer workflows as a primary interaction, not a hidden render
  command.
- External input, internal routing, groups, sends, and selected resampling
  sources as the graph model permits.

## Later, Not Scheduled

- MIDI input/output and controller mapping.
- Plugin hosting.
- Multiple time signatures and tempo maps.
- Stem export and compressed delivery formats.
- Groove extraction from audio.
- A text or algebraic pattern view.
- Platform support beyond Linux.

These are not rejected. They should not distort the current architecture until
the core instrument and buffer hypothesis work.
