# Current System

Status: implementation snapshot, August 2026.

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
- The complete 256-channel addressable bank. A new song starts with a lightly randomized four-channel
  drum kit (kick, snare, closed hat, and open hat); creating another new song
  generates a new variation. Channels can use the sampler, drum synth, mono
  synth, or poly synth and every rack row exposes mute, output volume, and
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
  fields, and a pinned velocity lane. Both axes use the zoom scrollbar —
  drag the thumb to pan, drag an end grip to zoom around the fixed end — in
  place of zoom-in/zoom-out buttons. The default pitch zoom starts three
  steps above minimum because that is where editing comfortably begins.
  It shares selectable straight/triplet
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
- A mixer sharing the work surface with the step grid, behind a Steps/Mixer
  toggle. It is a strip per bus - master first, then sixteen inserts - with a
  name plate, live stereo meter, fader, pan, mute, destination, and a count of
  the channels feeding it. Clicking a strip's name plate points the device rack
  below at that bus, so a chain on a group of channels is built with the same
  gesture as a chain on one channel. Channels name their bus from a picker in
  their rack row, beside their other output controls.
- A horizontal lower device rack with one fixed-height 3U source face followed
  by a chainable effect chain (filter, drive, bitcrush, delay, gate,
  compressor, and limiter; slots are added by kind from the rack's add slot,
  bypassed or removed from their shared host header, and reordered by dragging
  a header). Sampler, drum synth, mono synth, and
  poly synth faces share the same rack chrome and preserve their dimensions at
  narrow widths through horizontal scrolling. Sampler controls are divided
  into Sample, Voice, and Tone pages; mono controls into Osc, Amp/Filter, and
  Mod pages; poly controls add a VOICE page for polyphony and stereo spread;
  the drum face keeps family, character, shared shaping, and voice-specific
  controls visible together. Replacing a source does not change the channel's
  notes or mixer state. Closed and open hats share a choke group in the
  generated starter kit.
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
- A two-pane Preferences dialog with General, Audio, MIDI, and Appearance pages;
  General persists developer mode and reveals the presently empty Developer page.
  Appearance presets and custom accents preview live and persist on Apply or OK.
  Shared audio controls, tooltips, and master peak-meter ballistics.
- A traditional menu bar above the toolbar (`menubar.slint`): File, Edit,
  Pattern, Channel, View, and Help. Menus are declared where their window
  callbacks are in scope, so an item is one `MenuRow` line and a new action is
  one callback plus one line. Rows for features that do not exist yet (undo,
  clipboard, pattern/channel management) are present but disabled, marking
  where they will land. The File menu covers song, kit, and selected-channel
  save/load, the sample-embed toggle, export, and quit; Ctrl+O / Ctrl+S /
  Ctrl+Shift+S / Ctrl+E / Ctrl+Q mirror it. Help has an About dialog with the
  crate version. Song documents are inspectable versioned TOML files with
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
  `cargo run -p mooloop-ui --example control-gallery` shows every control; set
  `MOOLOOP_GALLERY_SNAPSHOT` and `MOOLOOP_GALLERY_SIZE` to capture it headlessly.
- The active interface contract is `docs/UI_DESIGN.md`. A visual composition
  tool using the real controls is available with `cargo run -p mooloop-ui
  --example mockup`; it saves and loads `mockup.toml` from the working directory.
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
selected source (sampler / drum synth / mono synth / poly synth) -> effect chain -> gain/pan/mute
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

Every strip preallocates all three source nodes and switches its active source
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
- `DrumSynth` (kick/snare/hat) and `MonoSynth` (three-oscillator, mono, glide)
  use the same timed note path as the sampler in realtime and offline renders.
- `EventList` carries fixed-capacity, sample-timed NoteOn, NoteOff, and generic
  ParamValue events.
- `StereoBus` ownership is centralized in the graph, leaving room for sends,
  groups, sidechains, and buffer taps.
- Musical time is PPQ 96, which exactly represents common subdivisions through
  64th notes and triplet grids.
- Realtime state is preallocated outside the audio callback. The channel and
  effect banks each cover their complete 256-value `u8` address space; these
  are bridge-format boundaries rather than small product caps.
- DSP tests cover sampler pitch, trim, loops, envelopes, filter behavior,
  reverse playback, and lo-fi stages, plus drum synth and mono synth voice,
  envelope, glide, filter, and LFO behavior. Mono synth tests also bound the
  largest sample-to-sample step across note retriggers and parameter changes,
  which is what the declicking work is defended by.
- The mono synth's LFO is one shape (sine, triangle, saw, square, or sample and
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
  sampler presets may carry a referenced or embedded WAV while synth presets
  contain only inspectable parameter state.
- Missing samples are recoverable by loading a replacement WAV, but there is no
  dedicated path-search/relink dialog, undo, autosave, or crash recovery yet.

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
  reclaim ring for control-thread destruction; reorder is an in-place slot
  swap, and knob changes arrive as sample-timed `ParamValue` events. Effect
  chains persist in song files
  (`ChannelSetup.effects`, serde-defaulted for older manifests).
- Nine effect kinds ship: a low-pass/high-pass filter, a drive/saturation
  with four curves at 2x oversampling, a bitcrush that is deliberately not
  oversampled, a stereo delay with damped cross-feedable feedback and
  digital/tape/reverse responses to a moving delay time, and a gate,
  compressor, and limiter sharing one detector and gain-computer module; a
  seven-band parametric EQ with optional bounded spectrum telemetry; and a
  generated-room convolution reverb. The reverb prepares a stereo room IR off
  the audio thread, then runs fixed-partition convolution with a host-aligned
  512-frame latency. Device faces are width-quantized in rack units; delay,
  gate, and compressor take 2U, while EQ and reverb take 3U.
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
- Modulation, automation, and a general parameter address type do not exist
  yet. `docs/MODULATION_PLAN.md` records the approved design for them.
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
