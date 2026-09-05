# Current System

Status: implementation snapshot, September 2026.

This document describes the prototype as implemented. It is deliberately
blunt about gaps so roadmap decisions are based on the system that exists.

## Implemented User Surface

- One application window with a two-row toolbar, channel rack, and a lower
  editor. The transport row carries play/stop, pattern-vs-song mode, a
  bar:beat:tick position readout, beat lamps, drag-or-type tempo, global
  sixteenth-note swing, and the master meter; the edit row carries pattern
  selection, the cursor tools, pattern length, and grid snap.
- Patterns are chosen with a fixed-width stepper plus a jump menu and can be
  named; the selector costs the same width at any pattern count.
- Four cursor tools drive the rack grid: Select (click toggles, ctrl-drag sets
  velocity), Paint (drag fills, right-drag clears), Slice (ratchet a step into
  2-4 even hits), and Stretch (drag a step sideways to set note length). The
  whole run of steps shares one hit area, because a per-cell one cannot follow
  a drag past the cell the press landed in.
- The complete 256-channel addressable bank. A new song starts with a lightly
  randomized four-channel drum kit (kick, snare, closed hat, and open hat);
  creating another new song generates a new variation. Channels can use any of
  six sources — the sampler, the drum synth, the v1 mono synth, the ML-M1, the
  v1 poly synth, or the ML-P8 — and every rack row exposes mute, output
  volume, and constant-power stereo pan.
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
- A horizontally and vertically zoomable piano roll with five pointer tools
  (Select, Draw, Paint, Slice, Erase; keys 1-5), a snap toggle (key 6), and
  exact pitch/velocity/length fields.
  - **Select** builds a selection: click, Shift-click, or drag a marquee
    across the grid. The marquee catches notes it overlaps rather than only
    those it encloses; Shift adds to the current selection and Ctrl+Shift
    removes. Double-clicking empty grid creates a note and drags its length.
  - **Draw** creates on a single click. **Paint** lays one note per cell it
    sweeps across. **Slice** cuts a note at the pointer, and with Shift held
    joins the selection instead, per pitch row. **Erase** deletes what it
    crosses; a right-drag does the same in any tool.
  - A selection behaves as one object. Pressing a note that is already
    selected keeps the selection, so the press can drag the group; the
    collapse to that one note still happens if the press turns out to be a
    plain click. Dragging moves the selection by a common delta, dragging
    either note edge changes every selected note's length by the same
    amount, and both clamp as a group so a chord keeps its shape. Notes have
    a left edge as well as a right: it moves the start and holds the end.
  - Alt and a note-edge drag stretches the whole selection in time about its
    opposite edge, lengths and gaps together, so doubling its span turns an
    eighth into a quarter. The pointer becomes an open hand over an edge
    that will stretch.
  - Copy-drag duplicates the selection in place and continues on the copy.
  - Selected notes are addressable from the keyboard: Delete removes them,
    the arrow keys nudge by the snap interval and transpose by a semitone,
    and cut/copy/paste act on notes rather than the channel whenever the
    roll has a selection. A paste lands the phrase after the selection,
    keeping its internal timing, and selects what it pasted.
  - The whole of a drag is one undo step, not one per pointer frame.
  - Both axes use the zoom scrollbar — drag the thumb to pan, drag an end
    grip to zoom around the fixed end — in place of zoom-in/zoom-out buttons.
    The default pitch zoom starts three steps above minimum because that is
    where editing comfortably begins. It shares selectable straight/triplet
    musical snap values from one bar through 1/64 with the playlist.
- Which modifier each roll gesture answers to is remappable in
  Preferences > Shortcuts: snap override, add to selection, remove from
  selection, copy on drag, and stretch. Defaults are Shift, Ctrl, Ctrl+Shift,
  Ctrl, and Alt. Shift is snap override alone -- it used to add to the
  selection too, which meant a Shift-drag deselected the note it was about to
  move and carried it off on its own. The snap override inverts the toggle
  rather than only defeating it, so it frees a drag when snap is on and
  quantises one when it is off.
- Two lanes sit under the roll and toggle independently: a velocity lane
  drawn as stems with drag heads, and one variable automation lane. The
  automation lane's picker lists every parameter of every effect on the
  selected channel and on every bus, grouped by device, with already-open
  lanes marked and clear/remove actions. Points are drawn by clicking,
  dragged to move, right-clicked to remove, and interpolate linearly. Lanes
  a clip is not currently showing are retained, not discarded.
- Sixteenth-note rack cells summarize their four 64th-note substeps without
  discarding rests between hits. Each substep is drawn solid where a note is
  struck and dim where one is merely held, so a ratcheted step is
  distinguishable from a single sustained note; coverage alone renders both as
  a full cell.
- A sampler editor with waveform, WAV/AIFF/MP3/FLAC/Ogg Vorbis loading and
  mixed-format sibling navigation, trim, reverse, root note, coarse/fine tune,
  loop region and mode, ADSR, low-pass filter with envelope depth and
  resonance, drive, bit reduction, and rate reduction. The filter runs its own ADSR, reached through a CURVE/ENV switch
  on the Tone page's filter panel; a patch that never sets one follows the
  amplitude envelope, which is what every project saved before it did. An
  Output trim in the page bar sets the patch's level ahead of the channel's
  inserts; a sampler created today starts at -9 dB so a normalized file peaks
  where the synths' default patches do, while projects saved before the trim
  existed load at unity. Voice controls cover one-shot/gated playback, 1-16
  voices, restart/layer retriggering, and 16 cross-channel choke groups.
- A mixer sharing the work surface with the step grid, behind a Steps/Mixer
  toggle. It is a strip per bus - master first, then sixteen inserts - with a
  name plate, live stereo meter, fader, pan, mute, destination, and a count of
  the channels feeding it. Clicking a strip's name plate points the device rack
  below at that bus, so a chain on a group of channels is built with the same
  gesture as a chain on one channel. Channels name their bus from a picker in
  their rack row, beside their other output controls.
- A horizontal lower device rack with one fixed-height 3U source face followed
  by a chainable effect chain (slots are added by kind from the rack's add
  slot, bypassed or removed from their shared host header, and reordered by
  dragging a header). Sampler, drum synth, v1 mono synth, ML-M1, and
  poly synth faces share the same rack chrome and preserve their dimensions at
  narrow widths through horizontal scrolling. Sampler controls are divided
  into Sample, Voice, and Tone pages; the v1 mono controls into Osc,
  Amp/Filter, and Mod pages; and poly controls add a VOICE page for polyphony
  and stereo spread. The ML-M1 is a distinct mono filter/performance instrument:
  Osc, Amp/Filter, and Perf pages expose separate amplitude and filter ADSRs,
  three low-pass filter characters, pre-filter drive, keytracking, a held-note
  priority stack, legato/retrigger and glide modes, and velocity Accent. The
  ML-P8 face is five pages -- OSC, NETWORK, FILTER, AMP and ML-P8 MOD, whose
  name distinguishes it from the frame's MOD button, which opens the channel
  shelf. It was one dense screen until 2026-09-04: at four rack units that fit
  sixty-nine parameters only at a 20px dial and a 9px caption, which is
  unreadable on a laptop, so the face spends a click per group and every
  control is a 34px `KnobStack` with its value still typed into. NETWORK is
  the source-by-destination grid with a page to itself, at 176px a column
  rather than 46. AMP carries the amp envelope beside allocation and
  character: Unison and Chorus as selectors under a fixed `VOICES 8` and the
  note count Unison leaves, with Detune, Spread, Drift and Glide as knobs. The
  face stays four rack units. The DS-01 face is six pages at four rack units
  on the same argument -- VOICE, TONE, NOISE, BODY, AMP and DS-01 MOD -- with
  every one of its ninety-two parameters on exactly one of them, at a 34px
  dial with its value typed into. It reads in the units a drum patch is
  written in: a time under a second is milliseconds, a frequency over a
  kilohertz is kilohertz, and a matrix route's depth is a signed percentage.
  A typed value means the unit it is written with, and with none written it
  means whatever the field was showing. A page is its layer's controls beside that
  layer's scope: the amplitude envelope carries the rendered hit inside its
  own contour, the pitch envelope is drawn over a quiet amplitude one for
  scale, and the eight-row matrix has a page to itself. The scopes are
  displays, not editors; their shared span is stated once in the page bar and
  follows the patch, so a 5 ms hat and a 4 s ride both read. The v1
  drum face keeps family, character, shared shaping, and voice-specific controls
  visible together. Replacing a source does not change the channel's notes or
  mixer state. Closed and open hats share a choke group in the generated
  starter kit.
- Every insert runs inside a shared device host. The host owns bypass, a
  generic dry/wet blend, independent input and output trims, insertion/removal actions, and separate
  held input/output peaks; its dry path is preallocated and runs after the
  device DSP, so parallel processing works even when an effect itself has no
  mix parameter. The dry path is delayed by the device's declared dry-path
  alignment latency before the blend, so latency-introducing effects do not
  comb-filter their own dry copy; wet-only returns may retain their own
  intentional pre-delay. Buses meter their effect slots the same way channels do: the
  rack polls whichever chain it shows, and a bus's head face reads its summed
  input and post-chain peak. Sources have a blank input meter because they generate rather
  than receive audio.
- Every gain trim — device input/output, the rack-row volume knob, the source
  output trim — is the same dB knob class: −60 dB (−∞) to +12 dB from unity,
  double-click to 0 dB. Project files and the engine wire keep linear gain.
- Every effect face inherits one shared shell (`EffectDeviceShell`): the
  identity header and drag-to-reorder live there, so a face file holds only
  its working controls and a new effect kind adds no chrome of its own.
- Source-device oscillator, lo-fi, and filter plots respond to their live
  parameters. Drum plots are generated by the production voice renderer;
  filter response geometry is reusable for LPF, BPF, and HPF modes.
- A two-pane Preferences dialog with General, Audio, MIDI, Appearance, and
  Shortcuts pages; General persists developer mode and reveals the presently
  empty Developer page, and the MIDI page is a placeholder with no controls
  on it yet. Appearance is seeded by three colors -- base (every
  neutral), accent (state), and alert (attention) -- with six built-in
  schemes, user schemes that can be saved and removed, and roundness and
  contrast scalars that retune the whole UI. All of it previews live and
  persists on Apply or OK. Shared audio controls, tooltips, and master
  peak-meter ballistics. A fresh install requests a 256-frame JACK buffer by
  default (Preferences > Audio picks from 64/128/256/512/1024/2048); a saved
  config that already has a buffer size choice keeps it, and the engine
  falls back to the server's current buffer size with a printed warning if
  the request is rejected. Sluggish input latency is a buffer-size symptom
  to check here before assuming a DSP bottleneck. The Shortcuts page lists
  every action in the registry (`ACTIONS.md`), grouped by category, each
  reassignable by clicking Record and pressing a key combination; rebinding
  and Reset/Reset All persist immediately, independent of the dialog's
  Apply/OK.
- A traditional menu bar above the toolbar (`menubar.slint`): File, Edit,
  Pattern, Channel, View, and Help. Menus are declared where their window
  callbacks are in scope, so an item is one `MenuRow` line and a new action is
  one callback plus one line. Rows disable themselves when they cannot act
  rather than being absent — Select All Notes is live only in the piano roll
  with notes on screen, Clear Pattern only when no project edit is pending.
  Every enabled shortcut shown on a menu row is one entry in the action
  registry (`ACTIONS.md`, `mooloop-ui/src/actions.rs`), which a single
  keyboard dispatcher in `main.slint` resolves and reassigns from the
  Shortcuts preferences page — 39 actions across transport, file, edit, note
  editing and pointer tools, pane switching and piano-roll zoom, channel, and
  pattern operations. The File
  menu covers song, kit, and selected-channel save/load, the sample-embed
  toggle, export, and quit; Ctrl+O / Ctrl+S / Ctrl+Shift+S / Ctrl+E / Ctrl+Q
  mirror it by default. Help has an About dialog with the crate version.
  Song documents are inspectable versioned TOML files with
  optional copied WAV assets in a sibling `.mooloop-assets` directory. Older
  directory-style song bundles remain loadable and migrate when resaved.
  Missing or corrupt samples warn and load as silent slots.
- Offline export of exactly one selected-pattern pass in Pattern mode or one
  derived playlist pass in Song mode, followed by a configurable 0-30 second
  release tail. Outputs are 24-bit PCM WAV, 32-bit float WAV, or 192/256/320
  kbps MP3.
- A shared widget library in `crates/mooloop-ui/ui`: knobs with value arcs and a
  bipolar mode (`controls.slint`), LED-segment metering with scales, latching
  clip indicators, gain-reduction and correlation meters (`meters.slint`), and a
  draggable graphical ADSR (`envelope.slint`). Source panels use bounded
  instrument modules, visible selector banks for short fixed choices, and
  horizontal or vertical parameter faders where aligned values need to use a
  module's area. Knob labels and value readouts share the knob's drag target.
  There is no standing sheet that renders every control at once; to see one,
  place it in the mockup tool below.
- The active interface contract is `docs/UI_DESIGN.md`. A visual composition
  tool using the real controls is available with `cargo run -p mooloop-ui
  --features mockup --example mockup`, or from Preferences > Developer in a
  build carrying that feature. It is off by default because everything one
  `.slint` entry point reaches compiles into a single generated Rust module,
  so exporting the tool from the window put 1.78 MB of generated Rust into
  every build; the Developer page hides the row when it is absent. Its palette comes from one
  catalog (`ui/mockup-catalog.slint`), grouped by role or module and filterable;
  items have z-order, a layers list, rack-unit sizing for device kinds, and a
  snap grid. Named layouts save to `layouts/` under the config directory, keyed
  by component name rather than palette index. Exported widgets with no catalog
  row show up in the palette's UNCATALOGUED group, which is the standing list of
  what the tool cannot yet compose with. That group is down to `PianoGrid` and
  `ModulationShelf`. The converse list -- UI patterns that recur but have no
  reusable component behind them at all -- is `docs/WIDGET_INVENTORY.md`.
- Some widgets exist ahead of the features that will use them: gain reduction and
  correlation have no audio behind them yet, and solo is a button style only.
- There is no metronome. The toolbar deliberately does not offer a click-track
  toggle, since nothing in the DSP graph produces one yet.
- MIDI input is wired but reaches nothing. The engine registers a JACK
  `midi_in` port and decodes a bounded number of messages per block into
  `mooloop_core::midi` types, and `RenderState` will apply a
  `BufferMidiMap` — note and CC mappings onto one Buffer insert's gestures —
  if one is installed. Nothing installs one: `EngineHandle::set_buffer_midi_map`
  has no caller outside its own tests, so decoded messages are dropped. There
  is no note input, no learn, no mapping editor, and no controls on the MIDI
  preferences page.

## Current Audio Path

```text
UI commands -> rtrb queue -> shared render state -> transport + sequencer
                                  |
                                  v
                         timed events/channel
                                  |
                                  v
selected source (sampler / drum synth / DS-01 / v1 mono / ML-M1 / ML-P8 / poly) -> effect chain -> gain/pan/mute
                                                           |
                                                           v
                                             assigned mixer bus (0-16)
                                                           |
                                        bus effect chain -> gain/balance/mute
                                                           |
                                             (optionally another bus)
                                                           v
                                                master bus (bus 0)
                                                           |
                                            master effect chain -> gain/pan
                                                           |
                                                           v
                                                       JACK outputs
```

The engine preallocates channel strips, pattern storage, event lists, and audio
buses. A JACK-independent render state owns transport, scheduling, instruments,
effects, mixing, and metering. The JACK adapter drains fixed-size commands into
that state and publishes position and master peak events; offline export drives
the same render path without JACK ports.

Every strip preallocates every source node and switches its active source
without allocating in the callback. WAV decode, waveform construction, and
directory scanning occur off the audio thread. A decoded sample is published
through an `ArcSwapOption` slot.

Project installation prepares a complete `RenderState`, including effect
construction and sequencer import, on the control thread. The JACK callback
receives that state through the ordered command stream, swaps one box at a
block boundary, and returns the displaced state through the reclaim ring for
control-thread destruction. Parameter commands cannot cross that generation
boundary.

## Useful Foundations

- `AudioNode` provides one in-place DSP interface for instruments and effects.
- `DrumSynth` (kick/snare/hat), `Ds01` and its one universal percussion voice,
  the v1 `MonoSynth`, the filter/performance-led `MlM1`, the eight-voice
  `MlP8` and its oscillator network, and `PolySynth` use the same timed note
  path as the sampler in realtime and offline renders.
  Their oscillator and envelope types are shared DSP primitives; their voice
  engines are deliberately separate.
- `EventList` carries fixed-capacity, sample-timed NoteOn, NoteOff, generic
  ParamValue, and internal-route-amount events.
- `StereoBus` ownership is centralized in the graph, leaving room for sends,
  groups, sidechains, and buffer taps.
- Musical time is PPQ 96, which exactly represents common subdivisions through
  64th notes and triplet grids.
- Realtime state is preallocated outside the audio callback. The channel and
  effect banks each cover their complete 256-value `u8` address space; these
  are bridge-format boundaries rather than small product caps.
- Sampler playback resamples through a band-limited windowed-sinc reader:
  unity rate is sample-exact, pitching up narrows the kernel's cutoff to
  keep foldback down, and the kernel folds across loop and ping-pong
  boundaries rather than filtering against silence.
- DSP tests cover sampler pitch, trim, loops, envelopes, filter behavior,
  reverse playback, and lo-fi stages, plus drum synth, v1 mono, ML-M1, v1
  poly, and ML-P8 voice, envelope, glide, filter, sync, and modulation
  behavior. ML-P8's sync aliasing is compared against an eight-times
  oversampled render rather than by looking for energy in a high band, since
  a hard-synced oscillator folds its alias products onto its master's own
  harmonic grid. V1 mono tests also
  bound the largest sample-to-sample step across note retriggers and parameter
  changes, which is what the declicking work is defended by.
- The v1 mono synth's LFO is one shape (sine, triangle, saw, square, or sample and
  hold) with a depth per destination: pitch, filter cutoff, pulse width, and
  tremolo. It free-runs across notes and silence unless set to retrigger.
- Mono synth amplitude is continuous by construction: the amp envelope attacks
  from its current level rather than restarting at zero, and velocity,
  oscillator levels, cutoff, and drive are one-pole smoothed over 5 ms so
  neither a retrigger nor a knob turn steps the waveform.

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

### Sampler Slicing And Stretch

Known gaps left open by the 2026-09 slice/commit push, each small enough to
land on its own when it starts to matter:

- **Live stretch is bypassed, not refused, in Slice mode, in reverse, and in
  Pong.** The DSP declines to run WSOLA backwards and the commit path is the
  answer, but the face still shows the ON toggle lit while nothing stretches.
  The sampler face has no `hover-hint` plumbing yet, so the explanation cannot
  reach the status bar from that toggle; adding the property to
  `SamplerDevice` and threading it through `main.slint` the way the effect
  faces do is the fix.
- **Auditions never fire a choke.** `inject_choke_events` is a pre-pass over
  the block's sequenced notes and runs before auditions are dispatched, so a
  slice auditioned from the face does not silence the rest of its choke group.
  A sequenced note in the group does still choke the audition. Making the
  pre-pass see auditions means queueing them before it rather than after.
- **Markers outside the committed region collapse onto its edges.** A commit
  renders only the playback region; a marker before it maps to frame 0 and a
  marker past it to the render's end, and the map then drops the duplicates.
  Revert restores every source marker exactly, so nothing is lost, but the
  published map after a commit holds fewer slices than the source had. Either
  the commit should refuse when markers fall outside the region, or the face
  should say how many it dropped.
- **A commit's spec is the whole render.** Nothing about the source file is
  checked on reload: a project whose referenced sample was replaced on disk
  re-renders the new audio under the old spec and lays the old markers over
  it. Recording the source's frame count in `SampleCommit` and treating a
  mismatch as a stale commit would catch this.

### Transport And Arrangement

- Pattern mode loops the selected pattern. Song mode layers playlist placements
  on the shared absolute clock and loops at the bar after the furthest clip end.
- Playlist starts use the shared musical snap while retaining absolute PPQ
  ticks and are bounded to a 64-bar start canvas. The timeline is horizontally
  zoomable. Global swing delays alternate sixteenth notes from 50% (straight)
  through 75% (strong shuffle), preserving note duration in realtime and
  offline rendering. There is no clip dragging, explicit song loop range,
  time-signature model, groove template, per-pattern swing override, or
  per-channel timing offset.

### State And Persistence

- `mooloop_core::Project` is the canonical serializable snapshot shared by the
  UI, realtime engine installation, persistence, and offline renderer. The
  live UI still owns incremental edits and produces snapshots for these paths.
- Songs, kits, and channel presets use the v1 bundle contract documented in
  `PROJECT_FORMAT.md`. Saves stage and replace bundles atomically; embedded and
  referenced asset policies are available per save.
- Channel presets are instrument presets for sampler and generated sources;
  sampler presets may carry a referenced or embedded audio file while synth
  presets contain only inspectable parameter state. They are saved and loaded
  from the channel row above the rack, because they span the generator and the
  channel's own state.
- A *device* preset is saved and loaded from that device's own rail in the
  rack -- the same two buttons on the generator and on every effect row. The
  load button offers only the presets saved for that device's kind, and is
  disabled when there are none.
- Missing samples are recoverable by loading a replacement audio file, but
  there is no dedicated path-search/relink dialog, autosave, or crash recovery
  yet.

### Mixing, Routing, And Effects

- Channel mute, volume, and pan are exposed, as compact knobs in the rack row,
  alongside the bus the channel feeds.
- Every channel names one mixer bus. The bank is the master plus sixteen
  inserts, all preallocated, so assigning a channel to any bus is a bounded
  mutation rather than an allocation. Buses carry their own effect chain,
  volume, pan, and mute, and may feed another bus.
- Any bus may feed any other. The realtime thread still never sorts a graph:
  `mooloop_core::compile_bus_graph` normalizes and topologically sorts the bank
  off the audio thread (Kahn's algorithm over fixed-size arrays, no allocation)
  and the engine walks the resulting `CompiledBusGraph`. This is the model
  REAPER and Ardour use - whoever edits the graph compiles it into a flat
  schedule, and the callback only executes that schedule.
- The schedule is this cheap because every bus owns a permanently allocated
  buffer and no two nodes ever share one, which removes the pooled,
  reference-counted buffer assignment a general graph engine needs.
- Destinations and their matching render order are one fixed-size compiled
  value. A routing change installs the whole value atomically, so no block can
  render edges against a stale order. Short stored banks are padded and invalid
  individual routes are repaired to the master at this compilation boundary.
- Cycles are refused rather than delayed, at the picker (looping destinations
  are shown greyed with the reason), at the command boundary, and on load,
  where a cyclic file is flattened to everything-to-master so it still opens
  and plays. Feedback routing would mean reading a bus's previous block, which
  is a deliberate feature rather than a fallback and needs a latency story this
  engine does not have.
- A muted bus still processes, so effect tails on it decay rather than freeze,
  but contributes no audio and meters as silent.
- Per-bus peaks reach the GUI through a shared array of atomics rather than the
  event ring, which the ring's drain rate could not keep up with. The published
  value is a peak hold that only the GUI's read clears, so a transient landing
  between two UI frames is still shown. Per-channel meters are still drawn but
  unfed.
- Channels retain the historical constant-power pan law, so existing project
  levels do not jump. Mixer buses use a distinct stereo balance law that is
  unity at centre and never boosts an endpoint; adding centred routing stages
  is therefore level-neutral.
- Each channel runs a full 256-slot addressable effect chain after its
  generator. Value edits, boxed structural edits, and prepared projects share
  one ordered control stream, so no edit can cross a project-generation
  boundary. Displaced nodes and whole render states return through a bounded
  reclaim ring for control-thread destruction; reorder is an in-place pointer
  rotation (`MoveEffect`), and knob changes arrive as sample-timed
  `ParamValue` events. Effect chains persist in song files
  (`ChannelSetup.effects`, serde-defaulted for older manifests).
- Structural edits keep every address honest. An effect is addressed by its
  slot and a channel by its index, so adding, moving, or removing a device --
  or deleting or pasting a channel -- is stated once as a permutation
  (`mooloop_core::structure`) and run over everything that names a position:
  the modulation matrix, every automation lane in every pattern, and the
  lane the editor is showing. The UI's model and the engine's mirror apply
  the same table for the same command, so a route or lane keeps meaning the
  device it was drawn on; a removed device takes its routes and lanes with
  it. Add, move and remove are undoable edits. The modulator grid follows the
  same rule one level down: a route aimed at a modulator's own parameter
  moves with that module and is dropped when its slot is emptied. On load,
  the integrity pass points a route or lane stranded on another channel's
  index back at its own channel and drops one that names a device or control
  that is not there, leaving addresses on a generator that has no descriptor
  table yet untouched.
- Twelve effect kinds ship: a low-pass/high-pass filter, a drive/saturation
  with four curves at 2x oversampling, a bitcrush that is deliberately not
  oversampled, a stereo delay with damped cross-feedable feedback and
  digital/tape/reverse responses to a moving delay time. Its Time control
  switches visibly between free ms and half/quarter/dotted-eighth/triplet/
  sixteenth divisions; while synced, every project BPM change immediately
  recalculates and sends the ordinary ms parameter to the audio engine. It
  persists the division, not just its current ms result. It is joined by a gate,
  compressor, and limiter sharing one detector and gain-computer module; a
  seven-band parametric EQ with optional bounded spectrum telemetry; a
  feedback-delay-network hall reverb; and one five-mode modulation processor
  (chorus, flange, phaser, ensemble, and ADT). Its delay-based modes share a
  bounded fractional stereo ring; Phaser uses a stereo all-pass cascade. The
  generic host supplies their dry/wet blend, so the DSP returns the processed
  signal only. The reverb runs eight modulated delay lines through a Hadamard
  feedback matrix behind a diffused, pre-delayed input, at a fixed per-sample
  cost independent of decay time and with no reported latency; Size, Decay,
  Damp, Pre, Diffuse, Width and Mod are all ordinary event-driven parameters,
  so every one of them is a working modulation destination. It replaced a
  generated-room convolution player whose per-block cost spiked over a
  64-frame budget at a two-second tail and which could not accept a parameter
  change at all without an off-thread IR rebuild. Beside it is a cheaper
  plate: eight parallel Freeverb-tuned combs into four series allpasses per
  channel, with Size, Decay, Damp, and Width, for material that does not need
  the hall. The twelfth kind is the retained-audio Buffer described below,
  which is an ordinary insert in the same picker. Device faces are
  width-quantized in rack units: filter, drive, bitcrush, limiter, plate, and
  Buffer take 1U; gate, compressor, EQ, and Mod take 2U; delay and reverb
  take 3U.
- Gate, compressor, and limiter share one transfer-curve display with a
  draggable threshold handle. Its live dot is fed by the device's own gain
  computer rather than by the surrounding peak meters: the audio thread
  reports the level its sidechain detector reached and the gain reduction it
  applied, held per block the way the peak meters are, and the display plots
  the dot at that detector level against the level actually leaving the
  device. So the dot moves with the attack and release the device is running,
  and rides above the static curve for as long as a slow release is still
  holding the gain down. Gain reduction is also read out three ways: a number,
  a rail down the right edge, and a warm glow over the whole plot whose
  strength tracks it. All three rest when nothing is coming in, so a gate shut
  on a silent channel does not sit lit up.
- The dynamics effects detect on the louder of the two channels and apply one
  gain to both, so compression cannot walk the stereo image around. The
  limiter has no lookahead on purpose: the engine has no plugin-delay
  compensation, so lookahead latency would shift a channel against its
  neighbours. Each kind publishes a static `ParamDescriptor` table
  (range, curve, unit, default) in `mooloop-core`, which is the single source
  of truth for normalization and clamping; `Event::ParamValue` carries natural
  units so nodes never handle curves. `EffectSlotState.params` is a tagged
  `EffectParams` enum, and the pre-tag untagged filter shape still loads.
- `mooloop-dsp`'s `delayline` module (`DelayLine` + `ReadHead`) is a shared
  ring primitive with cubic-Hermite fractional reads and crossfaded head
  jumps. The delay effect is its first consumer; the retained-audio buffer
  device is meant to be the second rather than growing its own ring.
- `ParamAddr` addresses parameters owned by a source, effect slot, modulator
  slot, or strip within its channel-or-bus scope. The per-channel `ModRack` and clip
  automation resolve through it. They compose rather than compete: a lane
  supplies the base a knob would otherwise supply, and the matrix adds its
  offsets on top, so an LFO wobbles around a drawn curve. Both resolve at the
  32-frame control rate into the destination's existing event path, and no
  effect needed a change to receive them.
- The channel modulation rack is a shelf under the device rack, collapsed by
  default. Open, it is a module grid beside the selected module's full
  surface. Five module kinds ship — LFO, Envelope, Step, Random, and Math —
  each a descriptor table plus a tick, so a module's parameters automate,
  undo, and persist like an effect's. Capacity is eight modules and sixteen
  routes per channel; the eight is a constant with a measured price rather
  than a layout assumption, and the grid scrolls to whatever it is set to.
  Routes carry durable `ModSourceId`s, so reordering the grid moves a module
  without changing what any route means, and `MoveModulator` remaps the Math
  module's `input_slot` across the same permutation. Arming a module's Assign
  switch makes legal controls assignable; dragging one sets route depth while
  the control keeps its base value. Removing a route restores the
  destination's base, on generator parameters as well as effect ones. The
  envelope's gate input is an explicit channel-note picker — the first
  adapter for a typed generator `Gate` outlet, which does not exist yet.
  Device outlets, cross-channel sources, and macros remain planned.
- The retained-audio buffer is descriptor-addressed: `Offset` places the read
  head behind the writer in beats and `Crossfade` sets the declick length.
  Offset is position mode, the same as a hand scrub — the head chases the
  position and the closing speed *is* the playback rate — so sweeping it is a
  scrub and holding it is delayed playback at unity. `bars` is deliberately
  not a parameter: resizing the ring reallocates, which happens off-thread.
  The JUMP/REV/STUT gestures are unchanged and outrank the offset while they
  run; the offset re-asserts on the next control tick after one ends.
- The sampler, v1 mono synth, ML-M1, ML-P8, DS-01, and poly synth are
  descriptor-addressed through `GeneratorParams`, so their parameters automate
  and modulate like an effect's. The three-oscillator synths reserve ten
  parameter ids per oscillator, starting at 100; ML-P8's and DS-01's ids are
  each their own namespace starting at zero, because neither is that voice
  with a different count. The **v1** drum synth is the one generator that is
  not addressable, and the reason is structural rather than unfinished work:
  `DrumSynthParams` is a mode-union, so a flat descriptor table over it would
  produce ids whose meaning changes with the Mode switch. DS-01 is the answer
  and is built: a second drum instrument beside the v1 device, addressable
  from its first commit, with the v1 device and its saved projects untouched.
  `docs/MODULATION_PLAN.md` records the approved design; build order is in
  `docs/plans/buffer-implementation/02-control-and-modulation.md`.
- DS-01 is a second drum instrument, not a rewrite of the first: one universal
  percussion voice with no drum-type mode, three layers — a morphing tone with
  a partial bank and FM, a four-colour noise generator through a morphing
  state-variable filter, and three tuned resonators that ring — into a shape
  stage with four drive characters. Four AHD envelopes with a curve control
  and an optional gate; a burst that fires up to eight impulses from one
  trigger inside one voice; and its own eight-row modulation matrix whose
  sources are per hit. It ships a factory bank of seventeen patches --
  three kicks, three snares including a velocity-shaped ghost, rim, clap, one
  tom at three tunings, both hats sharing a choke group, a gated ride,
  cowbell, clave and a zap -- seeded once into `presets/generators/ds01/` as
  generator presets, since a DS-01 patch's modulation is inside its own voice
  and has no channel rack to re-scope. Those are the same patches the DSP
  acceptance test asserts, so what ships is what is checked; **nobody has
  listened to them yet**, and the range tuning that follows from listening is
  the open half of the plan's last step. Its published outlets are not built
  and are blocked on the same device-outlet mechanism ML-P8's step 06 needs.
- **A device's published outlets can drive other devices.**
  `mooloop_core::outlet` states the vocabulary — control versus audio domain,
  the tap point an audio outlet is taken at, and the one-block latency every
  control outlet carries — and the ML-P8 declares fourteen outlets and
  publishes its seven control values, reduced through the group of its most
  recent note. A modulation route names its source through `ModSourceRef`, so
  it may be a rack module or a generator outlet; both resolve into one flat
  control address space, and an outlet route persists by outlet id. The
  latency is an ordering fact rather than a delay: the control table is filled
  before the strips render, so a route necessarily reads what the generator
  published in the previous block, live and offline alike.
  What is missing is the *surface*: the source picker does not list outlets,
  so such a route can be written by a project file but not built by hand. The
  audio outlets are declared and not connectable, pending typed audio edges.
- The ML-P8 has a device output stage: Volume and Pan, before the channel
  strip's own. They exist to be the base its per-voice `VcaLevel` and `Pan`
  modulation destinations offset from, which resolved from hardcoded unity and
  centre before them -- so a Velocity route on Pan now swings around wherever
  the patch put the device, and Spread widens around that rather than around
  the middle.
- The ML-P8 allocates its eight physical voices as *groups*. Unison at 1x, 2x,
  4x and 8x spends the pool rather than growing it, leaving 8, 4, 2 and 1 notes
  of polyphony; a note allocates a complete group and steals complete older
  groups, and a slot stolen by a smaller group leaves through the same short
  de-click transition rather than stopping. Changing Unison releases the
  sounding groups and applies the new topology to the next note; it never
  resizes a group in place. Detune and Spread place a group's members
  symmetrically about the note that was played, and at 1x Spread places notes
  by their stable slot positions so a chord occupies the field the same way on
  every render. Drift is one control over stable per-slot offsets to
  oscillator pitch, cutoff, the envelopes' attack, decay and release times, and
  oscillator start phase — never sustain, and never from runtime entropy, so
  Drift 0 renders bit-for-bit what the patch authored. A finishing chorus with
  four fixed policies (OFF, I, II, Ensemble) reuses the rack's modulation
  effect over ML-P8's own scratch buses, never the channel's; OFF is a true
  bypass and a mode change crosses through a silent wet rather than stepping.
  There is no gain normalization by voice count anywhere in the device.
- The ML-P8 and DS-01 are the two generators with modulation of their own. It owns an
  audio-rate LFO and a list of internal routes reading six per-voice sources
  — the LFO, both envelopes, velocity, key, and gate — into thirty-one
  continuous destinations, resolved per sample as authored base plus offset and
  clamped through the destination's own descriptor. This is deliberately not
  the channel shelf: the shelf's sources are per channel, and a polysynth needs
  values that differ between two notes held at once. A route's amount is
  automatable through `ParamOwner::SourceRoute`, addressed by the route's
  durable id rather than by a parameter of the device.
- Clip automation is per (pattern, channel), lives in the clip that drew it,
  and may address a bus. Two clips automating one destination is not
  prevented; the lowest channel wins at render time.
- Buses are insert points, not sends: a channel feeds exactly one, with no
  parallel send, return, or wet/dry split. There are no sidechains, external
  inputs, solo, or per-bus stem export, and buses cannot be renamed from the
  interface yet.
- There is no graph-level plugin delay compensation. `AudioNode` can report
  integer processing latency, and the drive declares the measured 15-frame
  latency of its complete 2x interpolate/decimate path. Drive also delays its
  internal dry path by the same amount, preventing its own wet/dry control from
  mixing time-misaligned signals. Related channels with unequal effect latency
  can still comb-filter when they meet at a bus; preallocated graph
  compensation must land before parallel sends.

### Buffers And Rendering

- A loaded sample is immutable in the audio path. The only audio the
  application generates for itself is a sampler stretch commit, which
  re-renders the decoded source off-thread under a stored spec, and the
  Buffer insert's rolling ring. Neither writes a channel's own output back
  into a project asset: there is still no capture-to-sample gesture.
- The render graph is independent of JACK and supports finite offline passes.
  WAV uses the active JACK sample rate; MP3 renders at 48 kHz through an
  in-process LAME encoder. Stem/bus export and realtime-vs-offline null testing
  are not implemented.
- Replaced sample lifetimes need a deliberate deferred-reclamation design so
  the last large sample allocation can never be freed on the realtime thread.

### Interface

- The application is usable but still has interaction and responsive-layout
  edge cases.
- One automation lane is visible at a time. Its picker reaches every
  parameter of every effect on the selected channel and on every bus, but
  several lanes cannot be shown at once, and the velocity lane is a separate
  fixed lane rather than one entry in that list.
- A canonical action registry drives the menu bar and rebindable shortcuts.
  Note multi-selection supports Select All and bulk deletion; channel,
  pattern, note, and modulation edits feed a project-snapshot undo/redo stack.
  Project-level navigation remains limited.

## Architecture Risks To Resolve Early

1. Add probability and explicit microtiming controls without weakening the
   tick-addressed event contract before broad automation.
2. Extend `docs/AUDIO_ARCHITECTURE.md`'s compiled plan from the current
   one-destination bus tree to typed audio and dependency edges before adding
   parallel sends or sidechains. An auxiliary-input processing view is also
   required; topology alone does not give an effect another audio input.
3. Budget channel buffer memory and specify read/write collision behavior
   before buffers become part of every strip.
4. Add deferred reclamation for replaced samples, graphs, and future buffers.

## Full Integration Verification

These commands are the release/integration suite, not the default checklist
for every change. Routine work should use the narrowest package, test target,
or snapshot that covers the behavior, as specified in `AGENTS.md`. Run these
commands sequentially when full integration coverage is warranted.

```sh
cargo test --workspace -j 2
cargo clippy --workspace --all-targets -j 2 -- -D warnings
cargo run -p mooloop-app --bin engine-selftest -j 2
MOOLOOP_AUTODRIVE=1 cargo run -p mooloop-app --bin mooloop -j 2
```

The `-j 2` cap is not optional on Adam's workstation, and neither is the
capped `[profile.dev]` debug info these rely on; see `AGENTS.md` for why.
