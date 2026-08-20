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
- Bring the piano roll closer to normal DAW behavior: fixed keyboard/header
  scroll sync, single-click selection without accidental note creation,
  double-click note insertion, double-click-drag length entry, and independent
  piano-roll snap.
- Establish consistent right-click removal and keyboard navigation.
- Make labeled knobs draggable from their labels as well as from the knob body.
- Finish sampler voice controls that do not require the new event model.
- Audit drum synth time ranges and parameter scaling; defaults and useful
  knob travel should make sub-100 ms percussion easy without making the rest
  of the range feel wrong.
- Add per-channel meters through the existing strip; volume and pan are exposed.
- Replace terse implementation-style tooltips with user-facing wording, and
  consider a status bar for longer hover text.
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
- Dragging the playhead should move transport position in the rack, piano
  roll, and playlist without creating a separate timeline concept.

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
- Cut, copy, and paste work consistently for notes, rack steps, playlist
  placements, patterns, channels, and parameter-lane selections through one
  command layer.
- Shortcut keys and right-click context menus are first-class command surfaces,
  not one-off handlers hidden in individual widgets.
- Extend the initial layered, tick-addressed playlist with clip dragging,
  selection, duplication, and keyboard editing on the shared musical grid.
- Add independent loop ranges to the existing Pattern and Song transport modes.
- Extend global swing with per-pattern overrides and groove templates built on
  explicit timing displacement.
- Extract the render graph from JACK and render offline to WAV.

Implemented foundation:

- The canonical project snapshot, atomic v1 song/kit/channel bundles, optional
  embedded WAV assets, missing-sample warnings, and transactional loading.
- A JACK-independent shared render state with one-pass WAV and MP3 export plus
  a fixed configurable release tail.
- Persistent global sixteenth-note swing shared by realtime and offline
  scheduling.

Remaining work in this phase includes autosave/recovery, undo, richer missing-
sample relinking, pattern management, playlist manipulation, cut/copy/paste,
shortcut keys, context menus, explicit loop ranges, per-pattern swing/groove
templates, and realtime/offline comparison tolerances.

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
- Restore effects only after the device and parameter contracts can support
  them without special-case automation or realtime graph edits.
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
- Add a mixer model where rack channels explicitly assign to mixer tracks.
  Unlike FL Studio, assignment and track management should be visible,
  deliberate, and recoverable instead of feeling like an afterthought.

Implemented foundation:

- DrumSynth and MonoSynth are selectable channel sources with complete editors,
  project/preset persistence, realtime playback, and offline rendering.
- New songs start from a randomized kick/snare/closed-hat/open-hat kit.

Remaining work in this phase is source-to-buffer workflow, external/internal
routing, groups, sends, and resampling-source selection.

## Later, Not Scheduled

- MIDI input/output and controller mapping.
- Plugin hosting.
- Multiple time signatures and tempo maps.
- Stem and bus export.
- Groove extraction from audio.
- A text or algebraic pattern view.
- Platform support beyond Linux.

These are not rejected. They should not distort the current architecture until
the core instrument and buffer hypothesis work.
