# Current System

Status: implementation snapshot, September 2026.

This document describes the prototype as implemented. It is deliberately
blunt about gaps so roadmap decisions are based on the system that exists.

## Implemented User Surface

- One application window with a transport toolbar, a work surface, and a lower
  dock. The transport row carries play/stop, pattern-vs-song mode, a
  bar:beat:tick position readout, beat lamps, drag-or-type tempo, global
  sixteenth-note swing, and the master meter, and never changes.
- **The work area is five views in three slots**, as of 2026-09-08. `main`
  and `split` divide the top; `bottom` is the dock. A view lives in exactly
  one slot, and a slot's tab strip lists what it holds, so no strip can
  misreport what is on screen. Each slot's rectangle is computed rather than
  nested in layouts, which is what lets a view be drawn anywhere off one
  instance.
- **The top pane splits.** The status bar's middle chip opens it with the main
  pane's other view; the divider between the two halves drags, resets to even
  on a double-click, and closes the split when dragged to either bound —
  folding its views back into the main pane rather than losing them. `View >
  Split Top Pane` does the same thing from the menu.
- **Any pane can fill the window.** Double-click a pane's active tab — the
  maximise gesture a title bar has, on the control that names the pane — and
  everything but the menu bar, the transport row and the status bar goes away.
  Double-click again or press `Esc` to restore; `View > Zoom Pane` and
  `Ctrl+Shift+\` do the same. Zoom never moves a view, so leaving it puts
  everything back where it was. The zoomed tab takes the full accent rather
  than the muted active fill, and the status bar says how to get out.
- **The bottom pane resizes for any view that does not declare its own
  height**, which is every view except `DEVICES` — a device face is a fixed
  268px and does not stretch. The playlist became resizable on 2026-09-08;
  before that the grip was live on the notes page alone.
- **Each view remembers its own dock height**, so switching tabs restores the
  height that view was left at rather than sharing one number.
- **The pane arrangement survives a restart**, in `[ui.layout]` of
  `settings.toml` beside the palette seeds: which slot each view is in, what
  each pane is showing, the divider position, each view's dock height, and
  whether the dock and the browser are open. Zoom is deliberately not saved —
  it is a glance, not an arrangement. An arrangement that could not be worked
  in falls back to the default panes and keeps the rest of the file.
- **A view moves between panes by dragging its tab.** Drop it on another
  pane, or on the right edge of an unsplit top pane to open the split there.
  The pane it would land in is tinted while the drag is live, the dragged tab
  dims at its origin, and the main pane's last view refuses to be dragged out
  — there would be nothing left to drop onto. `View > Move <name> to …` does
  the same from the menu, and so does right-clicking the tab itself — which
  is where a tab says what can be done to it, since the drag and the
  double-click have no affordance of their own. This is how the mixer reaches
  the bottom pane.
- **A view is revealed, not navigated to.** `Ctrl+1`..`Ctrl+5` and the `View`
  menu name `Steps`, `Mixer`, `Devices`, `Notes` and `Playlist`, and each
  shows that view wherever it lives. There is no longer an `editor page`: a
  page index of the lower dock could not name a view that had moved out of
  it.
- **A view has exactly one toolbar row, and its slot's tab strip leads it.**
  With `STEPS` up it carries pattern selection, the cursor tools and pattern
  length; with `MIXER` up, nothing, because the mixer's controls are on its
  strips; with `DEVICES` up, the device chain's source picker and, at the far
  end, the channel name and its preset browser.
  The dock used to stack **two** rows -- a header with the switcher, the
  channel name and the preset browser, and a per-page row under it. Merging
  them returned 34px and removed a `SONG ARRANGEMENT` label that named the
  pane a tab beside it already named. The preset browser is now on `DEVICES`
  alone; on the piano roll a whole-channel preset browser was noise.
  The pattern controls and the `STEPS/MIXER` switcher had already moved once,
  on 2026-09-07, off a 26px strip of their own.
- **Each editor owns its own grid snap**, in its own row: the piano roll a
  toggle and a menu, the playlist a menu. A third snap control used to sit in
  the toolbar and ask `editor-page` which of the two indices it was editing.
- The source device is a picker rather than a chip per instrument.
- Patterns are chosen with a fixed-width stepper plus a jump menu and can be
  named; the selector costs the same width at any pattern count.
- Pattern length moves a beat at a time with Shift -- on the STEPS field's
  arrows and wheel, and on `pattern.length-grow` / `pattern.length-shrink` --
  so sixteen steps to thirty-two is four gestures rather than sixteen.
- The rack grid's accent is a toolbar setting rather than a fixed four: 2, 3,
  4, 6, 8, 12 or 16 cells between bright ones, which is what makes a triplet
  or a 6/8 pattern readable on it.
- Four cursor tools drive the rack grid: Select (click toggles, ctrl-drag sets
  velocity), Paint (drag fills, right-drag clears), Slice (ratchet a step into
  2-4 even hits), and Stretch (drag a step sideways to set note length). The
  whole run of steps shares one hit area, because a per-cell one cannot follow
  a drag past the cell the press landed in.
- The complete 256-channel addressable bank. A new song starts with a lightly
  randomized four-channel drum kit (kick, snare, closed hat, and open hat);
  creating another new song generates a new variation. Channels can use any of
  eight sources — the sampler, the v1 drum synth, the DS-01, the v1 mono
  synth, the ML-M1, the v1 poly synth, the ML-P8, or Aux In, which plays
  another channel's published audio outlet — and every rack row exposes mute,
  output volume, and constant-power stereo pan.
- Channels can be reordered by dragging a rack row's name plate. The rows
  between the grab and the landing slide aside, and the gap that opens is the
  drop indicator. Every address in the song that named a channel follows it —
  automation lanes, modulation routes, an Aux In's subscription — and so does
  the session's own state: the selected device, the open automation lane, and
  the preset labels a channel and its rack rows are wearing. The move is one
  undoable edit.
- Patterns are created explicitly from a one-pattern project, with up to 256
  addressable pattern IDs and independent logical lengths from 1 to 256 steps.
  Hidden steps survive shortening and re-extending a pattern.
- Pattern and Song transport modes are independent of the visible editor.
  The playlist is a lower-pane tab, supports layered tick-addressed pattern
  instances, and remains editable while either mode plays. Clip width follows
  each pattern's natural length.
- A song loop repeats a marked section of the arrangement. The section is
  dragged out on a strip above the playlist's bar numbers, the Loop button and
  the L key switch it on and off without discarding its points, and the
  playhead is dragged along the bar numbers themselves.
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
- A mixer sharing the work surface with the step grid, behind the toolbar's
  STEPS/MIXER tab strip. It is a strip per bus - master first, then sixteen inserts - with a
  name plate, live stereo meter, fader, pan, mute, destination, and a count of
  the channels feeding it. Clicking a strip's name plate points the device rack
  below at that bus, so a chain on a group of channels is built with the same
  gesture as a chain on one channel. Channels name their bus from a picker in
  their rack row, beside their other output controls.
- A horizontal lower device rack with one fixed-height 3U source face followed
  by a chainable effect chain (slots are added by kind from the rack's add
  slot, bypassed or removed from their shared host header, and reordered by
  dragging a header). **A drag shows itself**: the dragged face lifts off the
  rack with a shadow and follows the pointer, the row it came from stays as
  an outline, and the rows between the grab and the landing slide aside to
  open a gap exactly one row wide where it will drop -- inside a container's
  box when that is where it lands. The landing is whichever row the pointer
  is over, measured from that row's own bounds, so a drag across a four-unit
  device counts it as four units wide. Sampler, drum synth, v1 mono synth, ML-M1, and
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
  visible together. The Aux In face is two rack units and one page, because a
  source, an outlet and a level is the whole device: two pickers, a Level
  knob, and a line saying where the signal is tapped from or why the
  subscription was refused. Replacing a source does not change the channel's notes or
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
- The generator at the head of a chain is selectable, by clicking its header
  the way a device row is selected, and wears the same border. It is the one
  rack row a click could not name. What it does not do is take part in
  copy, cut, duplicate or paste: those move rows of an effect chain, and a
  generator is not one -- moving a patch between channels is what its
  presets are for.
- Every effect face inherits one shared shell (`EffectDeviceShell`): the
  identity header and drag-to-reorder live there, so a face file holds only
  its working controls and a new effect kind adds no chrome of its own. The
  shell publishes the grab into the `RackDrag` global and reads the landing
  back out of it; the rack's rows work out which of them the pointer is over,
  because they are what knows the geometry.
- Source-device oscillator, lo-fi, and filter plots respond to their live
  parameters. Drum plots are generated by the production voice renderer;
  filter response geometry is reusable for LPF, BPF, and HPF modes.
- The window holds three pieces of furniture around the work area, none of
  which is a dockable-pane system: an always-visible docked status bar
  carrying hover hints and the panel toggles, a draggable splitter that
  resizes and collapses the lower editor dock, and a right-hand browser
  sidebar on a resize grip. The sidebar browser has two tabs over one row
  model, SAMPLES and PRESETS. **Samples**: persisted locations added through
  a folder picker and removed from a right-click, a tree flattened to one row
  per visible entry, filtering to playable formats, an autoplay arm and a
  preview-gain trim feeding a dedicated engine preview voice, an info pane
  with waveform, name, and format stats, and loading either into the selected
  channel or into a new one. **Presets**: every well-known preset directory
  scanned on entry to the tab and grouped — Channels, then one group per
  device kind, then one per effect kind, empty groups omitted — each group
  expanding to its presets with a count beside it, and a preset's category
  and tags shown when they say something its group does not. Clicking a
  preset loads it. A channel preset replaces the selected channel and a
  generator preset replaces its source device, which is why a generator
  preset is offered only on a channel already holding that kind and is drawn
  greyed otherwise. **An effect preset appends a device to the end of the
  selected channel's chain rather than replacing one**, so it is always
  loadable; the rack row's own rail is still where a preset replaces what is
  already in a row. Loading is one undoable edit either way. Neither tab can
  be driven from the keyboard.
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
  Shortcuts preferences page — 44 actions across transport, file, edit, note
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
  draggable graphical ADSR whose stages are times on their descriptor's own
  range and curve, not normalised positions (`envelope.slint`). Source panels use bounded
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
selected source (sampler / drum synth / DS-01 / v1 mono / ML-M1 / ML-P8 / poly / aux in) -> effect chain -> gain/pan/mute
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

Channels render in a compiled order rather than in index order, so a producer
runs before any channel subscribed to one of its audio outlets and the samples
arrive in the same block. A project with no subscriptions compiles to the
identity order and allocates no tap buffers, so the schedule is provably
inaudible until an edge is authored. The channel modulator tick pass stays in
index order and stays a separate loop: a modulator's phase must not depend on
a subscription somebody made on another channel.

Devices and channels with nothing to do are not rendered. A device says how
long it can still be heard after its input goes silent and whether its own
state has settled; the host stops calling an effect slot whose input has been
quiet longer than that, and stops rendering a whole channel strip when it has
no events, its generator has no voices and has been putting out silence, and
every effect on it would be skipped. Waking is the first block with audio in
it, from the state the device had when it stopped — nothing is reset and
nothing ramps. On a thirty-two channel project with one channel playing, a
256-frame block costs about a tenth of what it did; with every channel playing
it costs what it did before, because nothing is skipped. What a project
renders is unchanged either way, at any block size.

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
  answer, and the face still shows the ON toggle lit while nothing stretches.
  The toggle now says which of the three it is, and to commit, in the status
  bar; `StatusHint` reaches it from any face without the threaded property
  this entry used to ask for.
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
- A song loop repeats a section of the arrangement instead. It is stored with
  the song in absolute PPQ ticks, applies in Song mode only, and never applies
  to an offline render, which walks the arrangement once from the top. A loop
  reaching past the song's own end plays the part of it that exists, so
  shortening a song under a loop stops the loop rather than being refused.
  Every sounding voice is released at the loop point and at a seek, because
  the note-off it was waiting for is no longer on the way.
- The playhead can be moved with the transport running or stopped, snapped to
  the playlist's own musical snap. Stop still returns it to the start.
- Playlist starts use the shared musical snap while retaining absolute PPQ
  ticks and are bounded to a 64-bar start canvas. The timeline is horizontally
  zoomable. Global swing delays alternate sixteenth notes from 50% (straight)
  through 75% (strong shuffle), preserving note duration in realtime and
  offline rendering. There is no clip dragging, time-signature model, groove
  template, per-pattern swing override, or per-channel timing offset.

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
  disabled when there are none. The device's header then names the preset it
  came from, and keeps saying so after its knobs are moved. A label is dropped
  when the device wearing it goes: changing the channel's source, loading a
  channel preset over it, or opening a song or kit, which replaces the whole
  rack.
- Missing samples are recoverable by loading a replacement audio file, but
  there is no dedicated path-search/relink dialog, autosave, or crash recovery
  yet.

### Mixing, Routing, And Effects

- Channel mute, volume, and pan are exposed, as compact knobs in the rack row,
  alongside the mixer track the channel feeds.
- **The mixer is a list of tracks, and a track is made because somebody made
  it.** A new song opens with the master; the starter kit adds `Drums` and
  `Bass`, with its four drum channels grouped onto the first. `+` in the mixer
  adds a track, and a track's device face renames it or removes it — both
  undoable. Removing one falls anything routed to it back to the master rather
  than leaving it unheard.

  **There is no `+ Bus` and no `+ Send`.** What a track *is* — an ordinary
  track, a bus, a send return — is decided entirely by what routes into it.
  See `TERMINOLOGY.md`.
- Every channel names one mixer track. Tracks carry their own effect chain,
  volume, pan, and mute, and may feed another track. The addressable space is
  the master plus sixteen; strips are materialised per track as a project
  loads rather than preallocated, which `CAPACITY_POLICY.md` measures.
- **Sends.** A track can route a copy of itself to another track, in addition
  to its output. The track's face carries a `Send to…` picker that offers the
  legal targets, and each send it gains draws a row there: the target's name,
  where it taps, a switch, a remove, and a fader. The sends area draws exactly
  the sends that exist and scrolls when they outgrow the room — there is no
  ceiling on how many a track has.

  **A send is a route, not a kind of track.** The track at the far end is an
  ordinary track that happens to be fed by sends, which is what makes it an
  effects return; there is no return object and nothing to create. The starter
  kit opens with one: `Reverb`, fully wet, fed post-fader by `Drums` and
  `Bass`.

  Two tap points: **post-fader** (the default, so the send follows the track's
  fader) and **pre-fader** (after the track's devices, before its fader, so it
  holds its level while the fader moves). Both are after the chain, so they
  arrive at the same time. Mute silences a track's sends, pre-fader ones
  included.

  A send is **always linear** — analog sum is what a strip does to its own
  output, and a send is a feed into another strip's input. A send whose target
  already leads back would loop and is refused, greyed in the picker with the
  reason, under exactly the rule the output picker uses. Switching a send off
  is not the same as turning it down: it keeps its level and stays in the
  routing, so nothing re-times.

  Send levels are **smoothed**, per sample. They are the first gain at strip
  level that is: a fader still stamps its value per block.
- Each send is compensated on its own edge. A producer with a send reaches two
  summing points, which generally arrive at different times and are owed
  different delays, so a track feeding a latency-bearing return waits for it
  on its dry path and stays sample-aligned where the two meet again.
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
  value, and a track's sends travel with it as one command, so no block can
  render edges against a stale order or a send whose target the order has not
  been told about. Short stored banks are padded, invalid individual routes are
  repaired to the master, and a send naming a track that is gone is dropped, at
  this compilation boundary.
- A send orders its target after its source, the same way an output does, and
  a cycle closed through a send is refused the same way one closed through an
  output is.
- **Analog sum.** Any mixer track can be switched to sum into its destination
  through a non-linear encode, decoded at that destination together with
  everything else feeding it that has the switch on. The control is a small
  button at the foot of the strip, set apart from mute, drawing a straight
  line when it is off and a sine when it is on. The master has none, because
  it feeds nothing.

  **A track's switch, and only a track's.** A sequencer channel has none: the
  console this models puts its Channel stage on a mixer strip, and mooloop's
  mixer strip is a track (`TERMINOLOGY.md`). Several channels on one track
  therefore reach it linearly and the track encodes their sum, which is what a
  desk does with a group.

  There is no device to place and no bus to create: every summing point
  decodes, and the master is already one, so two channels switched on glue
  with nothing configured. Nesting needs no special case either — a
  console-on bus encodes at its own output and whatever it feeds decodes it.
  A strip switched on **alone** changes nothing, exactly; the character is
  entirely in the interaction between strips that opted in together.

  Off is the default and is bit-identical to a linear mixer. Switched on, the
  summing law separates a sparse mix and bounds a dense one at +3.92 dBFS —
  see `GAIN_STRUCTURE.md`, which records that ceiling as a deliberate
  exception to "nothing bounds a sample in the live path". A bus's fader sits
  after its decode, so pulling a bus down is level and pulling its feeders
  down is drive.

  Called *analog sum* in the interface and *console summing* everywhere in the
  source and the documents; the technique is the Airwindows Console idea, and
  `mooloop_dsp::console` is where the curve lives.
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
  digital/tape/reverse responses to a moving delay time. Its Time control is
  a knob with a sync lamp: dark, it sweeps free milliseconds; lit, it steps
  the same twenty-one-entry musical grid the modulators use, `4/1` down to
  `1/64T` with dotted and triplet entries throughout. While synced, every
  project BPM change immediately recalculates and sends the ordinary ms
  parameter to the audio engine, clamped to the two seconds the delay line
  can serve -- a division asking for longer reads in amber. It persists the
  division, not just its current ms result. It is joined by a gate,
  compressor, and limiter sharing one detector and gain-computer module; a
  seven-band parametric EQ with optional bounded spectrum telemetry; a
  feedback-delay-network hall reverb; and one five-mode modulation processor
  (chorus, flange, phaser, ensemble, and ADT) whose Rate carries the same
  sync lamp the delay does, over the same grid, clamped to the 12 Hz its LFO
  runs to. Its delay-based modes share a
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
  limiter has no lookahead. The reason it was built that way -- the engine
  had no delay compensation, so lookahead latency would have shifted a
  channel against its neighbours -- expired on 2026-09-05 when the mixer
  became latency compensated, and whether the limiter should now take
  lookahead is an open decision rather than a settled no. Each kind publishes a static `ParamDescriptor` table
  (range, curve, unit, default) in `mooloop-core`, which is the single source
  of truth for normalization and clamping; `Event::ParamValue` carries natural
  units so nodes never handle curves. `EffectSlotState.params` is a tagged
  `EffectParams` enum, and the pre-tag untagged filter shape still loads.
- `mooloop-dsp`'s `delayline` module (`DelayLine` + `ReadHead`) is a shared
  ring primitive with cubic-Hermite fractional reads and crossfaded head
  jumps. The delay effect is its first consumer; the retained-audio buffer
  device is meant to be the second rather than growing its own ring.
- A rack device may be a **container**: `EffectKind::Chain` holds an ordered
  run of the devices after it, appears in the rack exactly where a device
  would, and nests four deep. It is made either from the insert menu, like any
  other device, or by wrapping a device that is already there (the fourth
  button on its left rail). Inserting from a container's own `+` puts the new
  device *inside* it; inserting from a leaf's `+` puts it before that leaf.
  Appending to the end of a box is a drag rather than an insert, because the
  end of a run has to stay addressable as "after the container" -- for an
  empty box, "just inside" and "just after" are the same position, so which
  one is meant has to come from the gesture rather than from the index. Its one control is a dry/wet mix across the
  whole run, delayed to match that run's latency — the wet/dry that a
  *single* device has always had, applied to a group. Bypassing a container
  skips its run without moving the channel in time. **Nothing in the
  interface**: a device's left rail wraps it in a container, a container's
  right rail unwraps it, and dragging a device onto a row already inside a box
  puts it in that box, and **dropping onto an emptied box puts it back
  inside** -- an empty container's span covers no index, so its own row is
  the only thing there is to aim at and a drop on it means "into this".
  Dropping on a container that still holds something keeps meaning "before
  it". **The run is drawn as a box, and the box is the container's own
  chrome**: its input rail stands at the head, its *output* rail stands past
  the last device it holds, and the recessed space between them is what its
  devices sit in. Both rails and that space are one colour, darker than a
  device, so the container reads as the thing the faces are inside of; the
  devices inside keep their outline but not their fill, so they stay unified
  chunks sitting in something rather than cards floating on it. Every device
  wears the same 1px perimeter, drawn over its own rails and header rather
  than under them. The border hugs the faces rather than
  standing clear of them, and nesting reads from the stacked border lines
  rather than a colour per level. An empty container caps itself, so an empty
  box still looks like a box. A container wider than the viewport has no
  collapsed form yet.
- A container's face draws the **parallel split** the box cannot show: the
  signal entering, a dry lane straight across, a wet lane through a chip per
  device in the run, and the sum taken between them at a node that rides to
  the blend position as the mix knob moves. The run itself is not listed on
  the face -- it is the box immediately to the right of it. A container **saves and loads as one preset** — the box
  and everything in it, from the same rail every other device's presets live
  on. The modulation driving a run does not travel with it, because a route's
  source lives in the channel's rack rather than in the container; see
  `docs/plans/containers/00-status.md`.
- `ParamAddr` addresses parameters owned by a source, a rack device, a
  modulator slot, or the strip, within its channel-or-bus scope. A rack device
  is named by a durable `DeviceId` minted when it is inserted, so reordering,
  inserting into or deleting from a chain changes no saved address at all; the
  position is derived from the chain on each read. A modulator is still named
  by slot inside the rack, and a channel by index. The per-channel `ModRack` and clip
  automation resolve through it. They compose rather than compete: a lane
  supplies the base a knob would otherwise supply, and the matrix adds its
  offsets on top, so an LFO wobbles around a drawn curve. Both resolve at the
  32-frame control rate into the destination's existing event path, and no
  effect needed a change to receive them.
- The channel modulation rack is a shelf pinned to the bottom of the editor
  dock, under the device rack and outside its scroll, collapsed by default.
  It is therefore never wider than the window, however long the device chain
  grows, and the chain's horizontal scrollbar sits above it. Open, it is a
  module grid beside the selected module's full surface. Five module kinds ship — LFO, Envelope, Step, Random, and Math —
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
  A published generator outlet is a source in the same shelf, in its own pane
  beside the modules. Device (effect) outlets, cross-channel sources, and
  macros remain planned.
- The retained-audio buffer is descriptor-addressed: `Offset` places the read
  head behind the writer in beats and `Crossfade` sets the declick length.
  Offset is position mode, the same as a hand scrub — the head chases the
  position and the closing speed *is* the playback rate — so sweeping it is a
  scrub and holding it is delayed playback at unity. `bars` is deliberately
  not a parameter: resizing the ring reallocates, which happens off-thread.
  The JUMP/REV/STUT gestures are unchanged and outrank the offset while they
  run; the offset re-asserts on the next control tick after one ends.
- **Every generator is descriptor-addressed** through `GeneratorParams`, so
  their parameters automate and modulate like an effect's. The
  three-oscillator synths reserve ten parameter ids per oscillator, starting
  at 100; ML-P8's, DS-01's and the v1 drum synth's ids are each their own
  namespace starting at zero, because none of them is that voice with a
  different count. `docs/MODULATION_PLAN.md` records the approved design;
  build order is in
  `docs/plans/buffer-implementation/02-control-and-modulation.md`.
- The **v1** drum synth was the last one without a table, and the argument
  against giving it one did not survive being checked. It was called a
  mode-union whose ids would change meaning with the Mode switch;
  `DrumSynthParams` is a flat struct of named fields, each of which means one
  thing forever -- `kick_start_hz` is the kick sweep start whatever Mode says,
  which is exactly why the other modes' knobs are *retained* across a mode
  change rather than reset. It now has sixteen continuous controls and four
  selectors under ids of its own, and a modulation route and an automation
  lane both reach them.
  **What Mode selects is audibility, not meaning.** A route onto a kick
  control does nothing while the device is in Snare mode: the value is still
  written and still the one the patch authored, it is simply not heard. That
  is the same situation as a route onto a bypassed effect, which the
  application permits everywhere, so it is documented rather than
  special-cased -- suppressing it would be a second rule about when a
  parameter exists. Mode itself is automatable and is latched on a voice at
  its trigger, so a lane moving it changes the *next* hit rather than
  reshaping the one that is playing.
  The device is otherwise unchanged: three modes, the same sound, and old
  projects load exactly as before. DS-01 remains the better instrument and the
  reason the v1 device does not need to grow; it was never the reason the v1
  device could not have a table.
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
  acceptance test asserts, so what ships is what is checked. The bank was
  played on 2026-09-04 and raised no range corrections; it is there to prove
  the architecture reaches a kit from the controls rather than to be a curated
  bank. It publishes six control outlets — `Amp Envelope`, `Mod Envelope`,
  `Velocity`, `Note`, `Gate` and `Trigger` — reduced through the hit created
  by the most recent trigger, which stays the focus for its whole life and
  falls to zero rather than stepping backward onto an older hit that is still
  ringing. `Trigger` is the one a drum channel wants: one publication wide per
  hit, so a kick can duck a bass, open a gate, or fire an envelope on another
  device with no sidechain graph. `Gate` is honest rather than useful here —
  it answers "any hit is still waiting on its note-off", which is low for the
  one-shot patches most of the kit uses. Its four audio outlets (`Tone`,
  `Noise`, `Body`, `Pre-Shape`) are declared with frozen ids and tap points,
  and an Aux In channel can read any of them.
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
  The shelf offers them: a channel whose generator publishes control outlets
  grows an OUTLETS pane beside its module grid, one named chip per outlet with
  the same live meter a module tile carries. A chip selects and arms like a
  module, so the ordinary assign-then-drag gesture builds an outlet route, and
  the route it writes names the outlet by its durable id. An outlet has no
  editor, because the device that publishes it owns its behaviour; the pane
  beside it shows the declaration instead. A generator that publishes nothing
  has no pane at all rather than an empty one. The audio outlets never appear
  in that pane: they are not control sources, and `OutletDomain` is what
  refuses them structurally rather than a rule the picker remembers. An Aux In
  channel is where they are read instead.
  **The two kinds of source publish in different ranges, and a route's
  polarity is about the module convention.** A rack module always emits
  `-1..1`, and `Unipolar` lifts that into `0..1` so a one-way module rests at
  the destination's base. An outlet publishes in its *declared* range, where a
  unipolar one is already `0..1`, so an outlet route takes the destination's
  own default — `Bipolar`, which passes the value through. `Unipolar` on an
  outlet remains meaningful, but only for a genuinely bipolar one such as
  ML-P8's `LFO`.
- **A channel can play another channel's published audio outlet.** `Aux In`
  is a generator kind whose sound is one subscription: a source channel and
  one of its declared audio outlets, at a Level. The samples arrive in the
  block they were made in, not the one after, because the channel loop walks
  a compiled order that puts a producer before its consumers —
  `mooloop_core::compile_audio_graph`, beside the bus graph and the
  compensation plan. A block-sized delay was ruled out on purpose: its length
  would be the host's buffer size, so the same project would render
  differently at 128 and 512 frames.

  The useful case is the surprising one. An audio outlet declares where in the
  device it is tapped, and ML-P8's five source outlets are tapped *before*
  each source's own Level — so an oscillator turned down to silence in ML-P8's
  own mix still publishes, and an Aux In can play it while it stays absent
  from the producer's output. The face says `pre-level` rather than leaving
  that to be discovered. A muted producer publishes too: mute is a decision
  about what reaches the bus.

  **A tap exists only while somebody is subscribed to it.** ML-P8 declares
  seven stereo outlets and materialising them all would be 448 KB a channel;
  the compiler names the distinct (producer, outlet) pairs somebody reads, the
  buffers are allocated on the control thread and installed with the schedule
  they belong to, and a project that has never authored an edge holds none.
  Two channels reading the same outlet share one buffer.

  An edge that cannot resolve is refused and **kept**: a subscription naming a
  departed channel, a device that publishes no audio, an outlet that is not
  declared, a control outlet, an outlet tapped after its channel's effects, or
  a ring of subscriptions. The face says which, and a user who builds a cycle
  and then breaks it gets the edge back rather than authoring it again. Aux In
  publishes its own output, so Aux Ins chain — and that is what makes a ring
  constructible at all.

  It is not a send: the producing channel does not know it is being read and
  its own routing does not move. It is not a router either — one subscription,
  one channel, one outlet. Parallel sends and sidechain key inputs are still
  absent and are what the compiled edge model exists for next.
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
- The ML-P8 ships a factory bank of eight patches -- Init Saw, Crosswire
  Brass, Furnace Stab, Cold Metal, Sub Pressure, Servo Pad, Broken Choir and
  Wide Machine -- seeded once into `presets/generators/mlp8/` as generator
  presets, since an ML-P8 patch's modulation is its own routes and its own LFO
  and has no channel rack to re-scope. Seven of the eight run at Unison 1x
  with the chorus off, and five leave Drift at 0: the bank's job is to show
  that the *network* reaches eight sounds, so a patch that needed a duplicator
  to be interesting would not have proved it, and a test asserts the counts
  rather than a comment claiming them. Init Saw is the device default
  unchanged, because the gain contract is calibrated against exactly that
  signal. Those are the same patches the DSP acceptance test plays, so what
  ships is what is checked. Adam played it on 2026-09-05 and it raised no
  range corrections; the bar it is held to is the one he set closing DS-01's
  bank -- enough to prove the architecture reaches its range from the
  controls, not a curated bank.
- Clip automation is per (pattern, channel), lives in the clip that drew it,
  and may address a bus. Two clips automating one destination is not
  prevented; the lowest channel wins at render time.
- **The mixer is latency compensated.** Every device declares the frames it
  adds, the bus tree compiles into a per-producer delay, and each channel and
  bus waits by the difference before it sums — so two channels hitting on the
  same tick land in the same frame even when one carries an oversampled device
  and the other does not. Only Drive costs anything today (fifteen frames), so
  the audible effect is small; what it removes is the comb filtering that was
  worst exactly when two channels were most alike, and what it unblocks is
  parallel sends and sidechains, which are untrustworthy without it.
  Bypass keeps its device's latency — a bypassed node's signal goes through
  the same delay rather than past it — so A/B-ing an effect A/Bs the effect
  and not the timing. Removing the device is what gives the latency back. The
  plan is derived from the project rather than tracked alongside it, so no
  edit path can forget to update it, and an offline render compiles the same
  plan as a live one.
- Buses are insert points, not sends: a channel feeds exactly one, with no
  parallel send, return, or wet/dry split. There are no sidechains, external
  inputs, solo, or per-bus stem export, and buses cannot be renamed from the
  interface yet.
- Latency compensation is the mixer's own, not a hosted plugin's. `AudioNode`
  reports integer processing latency and `EffectKind` declares it without
  being built; the drive is the only kind that costs anything, at the measured
  15 frames of its complete 2x interpolate/decimate path, and it also delays
  its internal dry path by the same amount so its own wet/dry control cannot
  mix time-misaligned signals. Channels with unequal effect latency no longer
  comb-filter when they meet at a bus -- see the mixer entry above. There is
  no plugin hosting, so there is nothing else whose latency would have to be
  discovered at runtime rather than declared.

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
- **A shortcut fires from wherever focus happens to be.** Fixed 2026-09-07;
  the entry that stood here described the defect and got its cause wrong, so
  it is worth recording what the cause actually was. Slint delivers a key to
  the focused item and then walks *parent* items towards the window, and the
  root `FocusScope` was a **sibling** of the layout holding the UI rather than
  its ancestor — so it only ever received a key while it personally held
  focus, which is why clicking a neutral background was what made shortcuts
  start working. The scope now surrounds the UI, with `focus-on-click: false`
  so it does not swallow the presses that reach the controls inside it.
  Separately, `ToolButton` used to accept Space, and `ToggleButton`,
  `SegmentedControl`, the pane tabs and every mute button are built from it,
  so clicking any of them left a caret that re-fired that button instead of
  starting the transport. Space is the transport; Enter activates a focused
  button.
- Keyboard navigation exists in the piano roll and nowhere else. The arrow
  keys move an existing note selection but cannot build one, and the browser
  tree — either tab of it — cannot be reached or driven from the keyboard at
  all. `docs/plans/interface-iteration/04-the-keyboard-pass.md` owns this.
- Channels have no colour. There is no track-colour field in the UI, the
  session model, or the project format, so nothing in the rack, mixer, or
  playlist is colour-coded by channel.
- One automation lane is visible at a time. Its picker reaches every
  parameter of every effect on the selected channel and on every bus, but
  several lanes cannot be shown at once, and the velocity lane is a separate
  fixed lane rather than one entry in that list.
- A rack device can be selected, copied, cut, pasted and duplicated. The
  selection is a `DeviceId` rather than a slot, so it follows its device
  through a reorder and clears when the device is removed; clicking a device's
  header selects it and clicking it again clears. Copy takes a container's
  whole run, the same unit it is deleted and saved as, and strips identity, so
  a paste is a new device that sounds the same rather than the same device
  twice. A paste lands after the run it was dropped on, and a run's end
  boundary is outside a container, so pasting onto a box's last child lands
  beside the box rather than in it. Duplicate is on every rack row's left
  rail; all four are on Ctrl+Shift+C/X/V/D, and all but copy are undoable.
  **The clipboard does not carry modulation routes or automation lanes**: a
  route's source is a module in the channel's own rack, so it cannot follow a
  device to another channel. That is the question `docs/plans/containers/`
  reserved rather than answered, and this inherits its answer.
- A canonical action registry drives the menu bar and rebindable shortcuts.
  Note multi-selection supports Select All and bulk deletion; channel,
  pattern, note, and modulation edits feed a project-snapshot undo/redo stack.
  Project-level navigation remains limited.

## Architecture Risks To Resolve Early

1. Add probability and explicit microtiming controls without weakening the
   tick-addressed event contract before broad automation.
2. ~~Extend `docs/AUDIO_ARCHITECTURE.md`'s compiled plan from the current
   one-destination bus tree to typed audio~~ — done 2026-09-05. The audio
   edge exists: `compile_audio_graph` orders producers before consumers,
   refuses rings, and hands out the tap indices, and a device is given its
   auxiliary outputs for the duration of one process call rather than
   retaining them. **Dependency edges are still missing**, and they are what
   a sidechain needs: a signal that schedules a producer without being summed
   into the consumer. The processing view is half built for the same reason —
   a *generator* takes an auxiliary input today (that is what Aux In is);
   an effect does not, and topology alone will not give it one.
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
