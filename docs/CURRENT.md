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
- **A channel sidebar flanks the work area on the left**, as of 2026-09-13.
  It holds the selected **channel or track**'s name and colour -- following
  the same selection the device rack does, so the two cannot describe
  different things -- plus three MIDI rows for a channel: IN and CH are live,
  and OUT is inert because MIDI output does not exist. A channel also carries
  an **OUTPUT** picker naming the mixer track it feeds, as of 2026-09-19. It
  is the same edit the rack row's chip makes, reading the same model row, so
  the two cannot disagree -- but it says `Bus 3` where a 30px chip in a run of
  steps can only say an arrow and a number, and it is to hand when the step
  grid is not on screen. A track's destination is not there: it lives on the
  track's own mixer strip, beside the analog-sum switch it is a property of. A track draws no MIDI
  rows at all rather than disabled ones: disabled means "not configurable
  yet", which is true of OUT and would be a lie about a track, which has no
  MIDI input to configure. It is hidden until the status
  bar's leftmost chip opens it. For a track it also holds that track's
  **sends**: one row each with destination, level, pre/post tap, enable and
  remove, plus a picker that routes a copy to another track. They were an area
  of the track's device face until 2026-09-13, and moving them put the editor
  somewhere always to hand rather than behind whichever view the bottom pane
  was showing. The mixer strip keeps only the levels, as one bar per send
  reading `3 REVERB  -6.0`: drag it (relatively; Ctrl for fine), double-click
  for unity, Shift+click to switch the send off or on, Alt+click to remove it.
  The tap point and the remove button are the sidebar's alone. It resizes
  by its right edge between 180 and 400px, remembers its width, and edits
  whatever channel is selected rather than holding a selection of its own.
  The `reference/img/mooloop-1.0-mockup.png` panel also draws PLUGINS and
  MIXER tabs; those are second views of the rack and the mixer and are
  deliberately not built.
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
- **A track's rack reads in signal order**, as of 2026-09-13: its head (name,
  routing, polarity, then its channel strip's drive/EQ/comp, bundled into one
  row rather than two beside each other since 2026-09-14), its own devices as
  inserts, and its fader last. The fader used to be drawn first, inside the
  head face, because that face sits where a channel's *generator* sits -- the
  start of a channel's path and the wrong end of a track's. Nothing sounded
  different; the rack simply showed a stage before the stages that precede it.
- **The bottom pane resizes for any view that does not declare its own
  height**, which is every view except `DEVICES` — a device face is a fixed
  268px and does not stretch. The playlist became resizable on 2026-09-08;
  before that the grip was live on the notes page alone.
- **Each view remembers its own dock height**, so switching tabs restores the
  height that view was left at rather than sharing one number.
- **The pane arrangement survives a restart**, in `[ui.layout]` of
  `settings.toml` beside the palette seeds: which slot each view is in, what
  each pane is showing, the divider position, each view's dock height, the
  width of each side panel, and whether the dock, the browser and the channel
  sidebar are open. Zoom is deliberately not saved —
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
  end, the field that renames the channel and its preset browser.
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
  six sources — the sampler, the v1 drum synth, the DS-01, the ML-M1, the
  ML-P8, or Aux In, which plays another channel's published audio outlet —
  and every rack row exposes solo and mute, output volume, and constant-power
  stereo pan. The rack's `+` offers those six when adding a channel, which is
  the same list the source picker offers when changing one; its rows are
  ordinary menu rows, reading down a left edge like the rest of the
  interface's menus. The v1 mono and poly synths were retired from both on
  2026-09-22 (the ML-M1 and ML-P8 cover them): a song that uses one loads and
  plays unchanged, and its channel's picker still lists it while selected.
- Channels can be reordered by dragging a rack row's name plate. The rows
  between the grab and the landing slide aside, and the gap that opens is the
  drop indicator. Every address in the song that named a channel follows it —
  automation lanes, modulation routes, an Aux In's subscription — and so does
  the session's own state: the selected device, the open automation lane, and
  the preset labels a channel and its rack rows are wearing. The move is one
  undoable edit.
- Mixer tracks can be reordered the same way, by dragging a strip's name
  plate; the strips it passes slide aside. The master stays first: its plate
  selects and never drags, and a drop over it lands in seat 1. Everything that
  named the track follows it — channels routed to it, other tracks' outputs
  and sends, automation lanes, modulation routes and MIDI control bindings —
  the device rack stays on the moved track, and the move is one undoable edit.
  The Track menu's **Move … Left** and **Move … Right** do the same one seat
  at a time for the track the rack is editing, and are bindable in
  Preferences > Shortcuts. A control binding now also follows a channel
  reorder, insert or delete; before 2026-09-16 it stayed on the old seat.
- Patterns are created explicitly from a one-pattern project, with up to 256
  addressable pattern IDs and independent logical lengths from 1 to 256 steps.
  Hidden steps survive shortening and re-extending a pattern.
- **Channels, tracks and patterns can each be named, and the names are
  saved.** A channel is renamed on the `DEVICES` toolbar, a track on its own
  device face, a pattern in the transport toolbar. A channel or a track
  refuses a blank name, because its rack plate or its mixer column is the only
  thing identifying it; a pattern accepts one and reads as `Pattern N`
  wherever it is drawn -- the pattern menu and the playlist's gutter --
  because its number is beside it there. A channel keeps the name it was given
  when its source device is changed: only a channel still wearing the outgoing
  device's default name is renamed after the new one. **A pattern's name
  survived save and reload only from 2026-09-13**; before that the session
  held it and the project format had nowhere to put it, so reopening a song
  numbered every pattern again. Cloning a pattern gives the copy its name and
  colour, and cloning or deleting one leaves every other pattern's name on
  that pattern; until 2026-09-22 both shifted the names after it by one.
- Pattern and Song transport modes are independent of the visible editor.
  The playlist is a lower-pane tab, supports layered tick-addressed pattern
  instances, and remains editable while either mode plays. Clip width follows
  each pattern's natural length.
- A song loop repeats a marked section of the arrangement. The section is
  dragged out on a strip above the playlist's bar numbers, or moved by the
  grab handle on either of its ends; the loop toggle in the playlist toolbar
  and the L key switch it on and off without discarding its points, and the
  playhead is dragged along the bar numbers themselves. **A new song opens
  with its first two bars marked and looping off**, so the strip arrives with
  something on it to grab rather than needing to be discovered.
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
- How a Super (Meta/Win) press is read is a preference on that same page:
  separate keys, which is the default and what every binding assumes; Super
  acting as Alt, where either key presses an Alt chord; or Alt and Super
  swapped. It changes no binding -- only which physical key reaches one -- so
  every chord the page lists goes on saying what it said. It is for a desktop
  whose window manager takes Alt before mooloop sees it, and for a keyboard
  with the two keys transposed. Pointer gestures are unaffected: a gesture
  role can already be assigned Meta outright.
- Two lanes sit under the roll and toggle independently: a velocity lane
  drawn as stems with drag heads, and one variable automation lane. The
  automation lane's picker lists the selected channel's generator, every
  parameter of every effect on that channel, the channel's fader and pan
  (as "Channel strip", after its chain), and every effect on every bus,
  grouped by device, with already-open lanes marked and clear/remove
  actions. Points are drawn by clicking,
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
  STEPS/MIXER tab strip. It is a strip per track, master first, and a strip is
  **92 px wide with three faces**. The middle one is what you look at while
  mixing: name plate, live stereo meter, fader, destination, a count of the
  channels feeding it, and the analog-sum switch at its foot. A `‹` and a `›`
  in the strip's bottom row reach the other two -- sends to the left, the
  channel strip to the right -- and the arrow of the face you are on becomes a
  dot, so the row says where you are as well as where you can go. It is one
  strip at a time, so one track can show its EQ while the rest still show
  faders. The name, the meter and the fader do not turn, and neither does the
  column of four small controls beside the fader -- pan, solo, mute, polarity
  -- because an EQ is set by ear while watching what it does to the level and
  a send is set against the fader that feeds it. **A tall enough mixer stops
  paging and draws the whole strip**: given the room, a strip lays out drive,
  EQ, compressor, sends and then the meter and fader with its destination and
  analog sum beneath them, and the two arrows go away because there is
  nothing left to turn to. There is no zoom and no mode -- it is the pane's
  height and nothing else, so a mixer that is given a slot of its own or
  dragged taller shows more of every track at once. Below that height, a
  turned strip's fader takes whatever height its sections do not want rather
  than leaving it empty. Clicking a strip's name plate points the
  device rack below at that track, so a chain on a group of channels is built
  with the same gesture as a chain on one channel. Channels name their track
  from a picker in their rack row, beside their other output controls.
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
  Amp/Filter, and Mod pages; and poly controls add a VOICE page for polyphony,
  stereo spread and **mono mode**. Mono mode is not `Voices = 1`: a pool of one
  voice steals from itself, so releasing the newer of two held notes leaves it
  on the note that is no longer down. Mono mode gives the voice a held-note
  stack, a note priority of Last, Low or High, and a Retrig/Legato switch over
  whether an overlapping note restarts the envelope -- and a release falls back
  to whatever is still held as a pitch change rather than as a new note, in
  either switch position, which is what makes a trill work. `Voices` greys out
  while it is on. Overlapping notes glide and a note landing on a release tail
  jumps; that is one fixed rule rather than a second glide control. The ML-M1 is a distinct mono filter/performance instrument:
  Osc, Amp/Filter, and Perf pages expose separate amplitude and filter ADSRs,
  three low-pass filter characters, pre-filter drive, keytracking, a held-note
  priority stack, legato/retrigger and glide modes, and velocity Accent. The
  ML-P8 face is five pages -- OSC, NETWORK, FILTER, AMP and ML-P8 MOD, whose
  name distinguishes it from the frame's MOD button, which opens the channel
  shelf. It was one dense screen until 2026-09-04: at four rack units that fit
  sixty-nine parameters only at a 20px dial and a 9px caption, which is
  unreadable on a laptop, so the face spends a click per group and every
  control is a 34px `KnobStack` with its value still typed into. NETWORK is
  the source-by-destination grid with a page to itself; its four columns
  divide the face's width rather than holding a fixed 176px and centring the
  remainder. Each cell is a **horizontal** slider -- the bar and the drag both
  run along the cell's long axis, since a cell seven times wider than it is
  tall spent its length saying nothing and its height saying the value. A cell
  draws its modulation state the way a knob's ring does: an armed source marks
  every legal cell and shows the excursion its route would produce about the
  authored value, an unarmed one shows where the running sources have actually
  put the parameter, and a dot per incoming route. AMP carries the amp envelope beside allocation and
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
  intentional pre-delay. **Every host move ramps** (MOO-108, 2026-09-23):
  wet/dry, both trims and a container's Mix follow their controls through
  the mixer's 5 ms one-pole per sample, and bypass is a crossfade between
  the device's output and the bypassed path while the device keeps running,
  taken out of the path once the fade is 60 dB down (about 35 ms). A
  bypassed container fades its Mix to dry the same way before its run stops
  being called. A device coming back from bypass is told its held audio is
  stale (`Discontinuity::Seek`), so an un-bypassed delay starts empty rather
  than playing the repeats it held when it went out. Removing a device fades
  it out of the path first: the executor holds the removal, and the edits
  queued behind it, until the fade has run, or 100 ms at most for a chain
  that is not being processed. Installing is the same in reverse (MOO-172):
  an added device fades in along the bypass crossfade, and one installed over
  another waits for that one to fade out first. Loading an effect preset,
  or a container preset over a run, works that way too: the old device or
  run fades out and the loaded one fades in. A preset load used to rebuild
  the whole channel, cutting its voices and tails, and now it doesn't. The
  device keeps its identity, so its routes and lanes stay attached. A
  single-row preset loaded onto a container's own row still goes through a
  project install. `continuity_tests.rs` holds each of these to
  the family's step bound. Buses meter their effect slots the same way channels do: the
  rack polls whichever chain it shows, and a bus's head face reads its summed
  input and post-chain peak. Sources have a blank input meter because they generate rather
  than receive audio.
- Every gain trim — device input/output, the rack-row volume knob, the source
  output trim — is the same dB knob class: −60 dB (−∞) to +12 dB from unity,
  double-click to 0 dB. Project files and the engine wire keep linear gain.
  A new channel starts at 0 dB however it is made, and the strip's volume
  descriptor defaults there too (`DEFAULT_CHANNEL_VOLUME`); one added from
  the toolbar used to start at −1.9 dB.
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
- **The status bar holds a failure until it is read** (MOO-132). A warning
  or an error has its own segment at the bar's left, in amber or red, and
  stays there until it is clicked away or a notice of at least its weight
  replaces it -- the next "Device selected" goes to the ordinary line beside
  it instead of over it. A sample that will not decode, a load onto a channel
  that is not a sampler, a new-channel load with no room, a failed preview,
  a take that could not be written or has nowhere to land, and a device
  whose NaN output the master's guard is silencing all arrive there; before,
  each went to the log or was overwritten within a click. So do a device
  that panicked and was silenced, and "No audio": when the audio stops or
  never started, that notice stays after the Reconnect question is
  dismissed, and comes down by itself when the audio is running again.
  The bar's right end reads the audio callback once a second: its load as a
  share of the block budget, a dropout count in amber that a click resets,
  and a "not realtime" badge when the callback thread is ordinary rather than
  realtime; it shows "DSP –" while no audio is heard, rather than the null
  driver's timing. Hovering either segment explains it in the hint line.
  `status_bar::notify` in `ui/src/status_bar.rs` is the one door.
- **An edit the engine's command ring has no room for waits instead of
  being lost** (MOO-134). The pump asks `EngineHandle::command_room()` before
  each message and holds what would not fit, in order, with everything
  queued behind it (`mooloop_session::engine::EngineBacklog`), delivering it
  first on a later tick. A parameter change used to be logged and dropped,
  and a refused channel edit said "waiting for audio" and was then thrown
  away with its undo step. Two installs waiting together merge into the
  newest, keeping the older one's undo step; a load or a Reconnect clears
  what was addressed to the old song. A backlog lasting a second raises a
  warning in the status bar -- "the audio engine has stopped" when the
  callback is not running, which is what a full ring usually means -- and
  it comes down when the backlog drains.
- The window holds three pieces of furniture around the work area, none of
  which is a dockable-pane system: an always-visible docked status bar
  carrying hover hints and the panel toggles, a draggable splitter that
  resizes and collapses the lower editor dock, and a right-hand browser
  sidebar on a resize grip. The sidebar browser has two tabs over one row
  model, SAMPLES and PRESETS. **Samples**: persisted locations added through
  a folder picker and removed from a right-click, a tree flattened to one row
  per visible entry, filtering to playable formats, an autoplay arm and a
  preview-gain trim feeding a dedicated engine preview voice -- a preview the
  command ring refuses says so in the status bar rather than being silence
  with no explanation. The voice plays a file at its own sample rate,
  band-limited like the sampler, so an audition is at the pitch the file
  will have once loaded; one that is stopped or replaced fades over 2 ms
  rather than cutting off -- an info pane
  with waveform, name, and format stats, and loading either into the selected
  channel or into a new one. The sampler face's prev/next-sample arrows step
  through **the folder the sample was browsed from**, which a save does not
  move: embedding a song rewrites where the bytes are, and before 2026-09-14
  it took the arrows with it, so "next sample" on a kick loaded the snare out
  of the song's own bundle. A song opened from disk has no browse folder to
  remember and steps through its bundle, which is all the document knows. **Presets**: every well-known preset directory
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
  already in a row. **Every preset load is one undoable edit**, effect,
  channel and generator alike, and so is a kit load; none of them stops the
  song, because each edits the song that is playing rather than opening
  another (MOO-95). Neither tab can be driven from the keyboard.
- A two-pane Preferences dialog with General, Audio, MIDI, Appearance, and
  Shortcuts pages; General persists developer mode and reveals the presently
  empty Developer page. The MIDI page lists the inputs the driver is offering,
  every controller mapping in the project, and all seven transport gestures
  with whatever is mapped to each. A mapping row can be relearned, removed,
  switched between pickup and jump takeover, and inverted. Relearning moves the
  row to the next control touched, keeping its takeover and direction when the
  new control is the same kind, and the old control stops driving the
  parameter; cancelling leaves the row as it was. A transport gesture
  is learned from its own row, since it has no on-screen control to press. One
  preference lives there, *bind to the controller it hears*, which is off by
  default and decides whether a learned mapping listens to one controller or
  to any. **Appearance is a theme, a variant, and a set of scalars.** A theme
  is a sixteen-colour ramp -- the interchange form base16, pywal and wallust
  all publish -- or, equivalently, the three seeds (base, accent, alert) the
  page's colour pickers write, which synthesize one. Sixteen ship:
  **Mooloop, Dracula, Nord, Gruvbox, Everforest, Solarized, Catppuccin, Tokyo
  Night, Rosé Pine, Monokai, Graphite, High Contrast, Ember, Indigo**, most
  with both of their published light and dark variants, and two homages,
  **Platinum** (after Mac OS 8) and **Impulse** (after Impulse Tracker), which
  bring square corners and a bevel with them. A fourteenth row,
  **Wallpaper**, appears whenever pywal or wallust has left a palette in
  `~/.cache/wal/` or `~/.cache/wallust/`, and is that palette. A variant the
  scheme does not publish is derived from the one it does and says so in the
  list. A **Dark / Light / Auto** control picks the side, where Auto follows
  the desktop's own `org.freedesktop.appearance color-scheme` (the system
  appearance on macOS), **live**: switching the desktop between light and dark
  switches mooloop within a frame, with Preferences open or closed (MOO-156).
  **The channel, track and pattern swatch palette follows the theme**: eleven
  colours taken from the ramp's eight hues plus a midpoint in each of its
  three widest gaps. A song stores the colour it was given rather than a
  palette index, so changing themes never repaints anybody's channels.
  Beside the colours: roundness, contrast, **text size**, **density**,
  **border and emphasis widths**, **relief** (flat, bevel or inset, with a
  depth; so far drawn by the buttons, the toggles, the segmented selectors and
  the tabs, and taken away again by a theme that doesn't state one), and
  **two font families** -- one for the
  interface and one for readouts. A font that is not installed falls back to
  the platform default, because Slint has no runtime font registration and a
  theme can only name a family. Themes save to
  `<config>/mooloop/themes/<name>.toml`, one file per theme, and a malformed
  one is skipped with a message rather than stopping startup. All of it
  previews live and persists on Apply or OK. Motion speed's **Instant** is labelled as
  reduced motion and is the default, so nothing animates unless asked to. Shared audio controls, tooltips, and master
  peak-meter ballistics. A fresh install leaves the buffer size where the
  driver has it, and Preferences > Audio shows that size; picking one
  (64/128/256/512/1024/2048) requests it from then on -- server-wide under
  JACK, the output device's own under Core Audio. A saved config that
  already has a buffer size keeps it, which includes an older install's
  256, written when that was the default. The engine falls back to the
  driver's current buffer size with a printed warning if a request is
  rejected. Sluggish input latency is a buffer-size symptom
  to check here before assuming a DSP bottleneck. The Shortcuts page lists
  every action in the registry (`ACTIONS.md`), grouped by category, each
  reassignable by clicking Record and pressing a key combination; rebinding
  and Reset/Reset All persist immediately, independent of the dialog's
  Apply/OK. A **Context column** beside each chord says where it applies —
  blank for the global majority, so the column marks the exceptions rather
  than restating the rule sixty-three times, and "Focused panel" means the
  chord asks what was clicked last. The recorder accepts an unmodified key;
  it refused one until 2026-09-14, which had made the shipped bare-L and
  bare-digit defaults impossible to put back after a Reset.
- A traditional menu bar above the toolbar (`menubar.slint`): File, Edit,
  Pattern, Channel, View, and Help. Menus are declared where their window
  callbacks are in scope, so an item is one `MenuRow` line and a new action is
  one callback plus one line. Rows disable themselves when they cannot act
  rather than being absent — Select All Notes is live only in the piano roll
  with notes on screen, Clear Pattern only when no project edit is pending.
  Every enabled shortcut shown on a menu row is one entry in the action
  registry (`ACTIONS.md`, `mooloop-ui/src/actions.rs`), which a single
  keyboard dispatcher in `main.slint` resolves and reassigns from the
  Shortcuts preferences page — 63 actions across transport, file, edit,
  arrow-key navigation, note pointer tools, pane switching and piano-roll
  zoom, channel, track, device, browser, and pattern operations. The File
  menu covers song, kit, and selected-channel save/load, the sample-embed
  toggle, export, and quit; Ctrl+O / Ctrl+S / Ctrl+Shift+S / Ctrl+E / Ctrl+Q
  mirror it by default. Help has an About dialog with the crate version.
  Song documents are inspectable versioned TOML files with
  optional copied WAV assets in a sibling `.mooloop-assets` directory:
  samples in its `samples/`, recorded takes in its `recordings/`. **A save
  adds to that directory and never rebuilds it** (since 2026-09-22): a file
  already there keeps its place and its name and is not copied again, and a
  file the song stops using stays until File > Clean Up Takes moves it to
  the trash, so an undo back to it still finds it. Older
  directory-style song bundles remain loadable and migrate when resaved.
  Missing or corrupt samples warn and load as silent slots. **Renaming a song
  and its assets folder together, in a file manager, works**: the document
  still names the old folder, and the loader reads this song's own instead and
  says so, which the next save writes back. Until 2026-09-14 that pair of
  renames made the song permanently unopenable.
- Offline export of exactly one selected-pattern pass in Pattern mode or one
  derived playlist pass in Song mode, followed by a release tail that runs
  until every device has fallen silent (`RenderState::is_at_rest`, so a
  reverb's decay and a delay's last echo are waited for), capped at a
  0-30 second limit the dialog sets (10 s by default). Outputs are 24-bit
  PCM WAV, 32-bit float WAV, or 192/256/320 kbps MP3. It renders in 512-frame blocks, a size live playback runs at,
  rather than the graph's 8192-frame maximum, where one automated parameter
  filled a device's event list and every later one on it was dropped from
  the export. Parameter events that still find no room are counted, and an
  export that lost any logs how many. The render passes through the same
  output guard as playback, so a file never holds NaN or a sample over
  0 dBFS; `RenderSummary` counts the mix's overs (which the safety limiter
  held at the ceiling), the non-finite samples written as silence, and any
  sample the 24-bit encoder still had to clamp, and a non-zero count is
  logged. As of 2026-09-23 (MOO-125) the export dialog stays up through the
  render with a progress bar and a Cancel, which stops the render and leaves
  any file already at the target untouched, and then shows the file's length
  and rate and every one of those counts. A finished file replaces the
  target with one rename. Every format renders at the session's rate: an MP3
  of a session faster than 48 kHz is rendered at the session's rate and
  converted by LAME as it encodes (88.2/176.4 kHz to 44.1, the rest to 48).
- A shared widget library in `crates/mooloop-ui/ui`: knobs with value arcs and a
  bipolar mode (`controls.slint`), LED-segment metering with scales, latching
  clip indicators, gain-reduction and correlation meters (`meters.slint`), and a
  draggable graphical ADSR whose stages are times on their descriptor's own
  range and curve, not normalised positions (`envelope.slint`). Source panels use bounded
  instrument modules, visible selector banks for short fixed choices, and
  horizontal or vertical parameter faders where aligned values need to use a
  module's area. Knob labels and value readouts share the knob's drag target.
  Every `ParameterKnob` has a right-click menu -- Type a Value, Reset to
  Default, MIDI Learn, Automate -- and takes a typed value: from the menu,
  Enter or F2 on a focused knob, or a digit typed at it. A typed value is read
  against the parameter's descriptor where the knob can name its parameter
  (`440`, `4.4k`, `A4` and `250 ms` all read), otherwise in the readout's own
  units; Escape leaves it alone. The menu and the typed entry are not yet on
  `MiniKnob`, the faders or the time knobs (MOO-143's follow-ups).
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
- Some widgets exist ahead of the features that will use them: correlation has
  no audio behind it yet.
- There is no metronome. The toolbar deliberately does not offer a click-track
  toggle, since nothing in the DSP graph produces one yet.
- A MIDI keyboard plays the selected channel, on every MIDI channel, whether
  or not the transport is running. Under JACK the engine's `midi_in` port is
  connected to every physical MIDI source at startup and to each one that
  registers later; under Core Audio every Core MIDI source is listened to
  through `midir`, and a keyboard plugged in later is picked up within a
  second. JACK notes keep their frame offsets; Core MIDI notes act at the top
  of the next block. A key comes up on every channel it went down on, so
  moving the selection while holding one does not strand a note. Note-on and
  note-off play, with velocity passed through. **The sustain pedal (CC 64)
  holds released keys** on every source, since it defers the release
  rather than asking the device (MOO-128). One pedal serves every input, and
  lifting it releases each held note on the channels that played it. A
  recorded note still ends when its key comes up. **The bend wheel bends**
  every pitched source (all but Aux In) by up to ±2 semitones, a fixed
  range that is not yet a setting (MOO-128). It reaches the channels a key
  from the same input would play, moves notes that are already sounding, and
  holds until the wheel moves again, so a note started with the wheel up
  starts bent. A channel keeps its bend when the selection moves away from
  it; Panic returns every channel's bend to centre. Bends are live only: they
  are not recorded into patterns and an export does not hear them. **The mod
  wheel (CC 1) and aftertouch are modulation sources** on every channel,
  whatever its instrument: the shelf's outlet band lists *Mod Wheel* and
  *Aftertouch* after the generator's own outlets, and they arm and route to
  any knob the way an outlet does (runtime slots 16 and 17, saved as
  `performance = 0` or `1` on the route). Aftertouch is channel pressure or,
  from a keyboard that sends it per key, the hardest-pressed key's pressure.
  Both reach the same channels a bend does, stay where the keyboard left
  them when the selection moves, return to rest on Panic, and are live
  only, like bends. Aftertouch is not learnable and is not forwarded to the
  control layer; CC 1 still is. Program change still does nothing on a
  channel. A `BufferMidiMap` — note and CC
  mappings onto one Buffer insert's gestures — takes the notes it maps ahead
  of the keyboard, but nothing installs one:
  `EngineHandle::set_buffer_midi_map` has no caller outside its own tests.
- **Per-channel MIDI input, controller mapping, transport control and MIDI
  recording.** `docs/plans/archive/midi-control/` landed on 2026-09-15. A channel picks
  its input and an Omni-or-1–16 channel filter from the sidebar's IN and CH
  rows, and the engine routes notes by them (only since 2026-09-17: until
  then every project install, the startup one included, handed the audio
  thread a renderer that read a private routing nobody wrote, so every channel
  behaved as Follow Selection); a stored port that is not plugged
  in says so under the picker rather than leaving the channel silently
  unplayable. A record-arm button sits beside play and stop — arming, not
  recording, so arming while stopped works — and an armed transport captures
  played notes into the pattern selected when the key went down, even if
  the selection moves before it comes up. In pattern mode a note lands where
  it was played in the loop (until 2026-09-17 every note after the first pass
  stacked on the pattern's last tick); in song mode it lands at its offset in
  the placement of the selected pattern under the playhead, and a note played
  where no placement of that pattern is playing is heard but not recorded.
  The transport follows an external Start,
  Continue, Stop or Song Position without any mapping, because a device that
  sends Start is asking for exactly one thing. Start plays from the
  beginning, Continue from where it paused, and Stop pauses.

  **LEARN** beside the transport arms controller mapping: press any knob or
  fader and then move a control on the desk, and the two are bound. The button
  reads LISTENING while it waits, and names the parameter it is waiting for in
  its tooltip — the status bar cannot hold that, because a hover hint outranks
  a status message there. The arm stays on, so a desk is mapped control after
  control without reaching back to the toolbar; the status bar names what was
  just bound. While it is armed
  every parameter control carries a ring, the pointer is a crosshair, and a
  press names the control rather than moving it — mapping a knob does not
  change the value it is about to follow. It reaches the same parameters
  modulation does: a device's own controls, the generator's, and the channel
  strip's volume and pan. **A pad or a key can be learned too**: while a
  learn waits, every key a keyboard or pad controller sends goes to the learn
  rather than to an instrument, and a key that is bound afterwards fires its
  target instead of playing a note (a pad defaults to a toggle, or fires a
  transport gesture). Every other key still plays. A mapped control takes over on **pickup** by
  default, so a fader left at zero does not slam a filter shut the first time
  it is touched, and a control that has taken over gives the parameter back
  the moment anything else moves it. A mapped fader on a channel or track
  volume follows the mixer fader's own taper, so unity sits at
  three-quarter travel and the top is +6 dB, as it is under the mouse; a
  volume lane and a volume modulation route use the same taper, and no
  volume control (rack-row knob and rail trim included) goes past +6 dB.
  Songs saved before 2026-09-23 are converted on load and play at the gains
  they were saved at, except that volume lane points above +6 dB clamp to it.
  The map is reviewed and edited on
  Preferences > MIDI, and it is saved with the project.

  **MIDI output does not exist**, so the sidebar's OUT row is still inert
  (`SCOPE.md` §2 item 2). **None of it has been run against a MIDI device**:
  every layer is tested and the application compiles, and no keyboard has been
  plugged into it.

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
                                            master effect chain -> master bus compressor -> gain/pan
                                                           |
                                                           v
                               output guard: NaN/Inf -> silence, 0 dBFS limiter (lookahead 0-5 ms)
                                                           |
                                                           v
                                  driver output (JACK ports or a Core Audio device)
```

**The output guard** (MOO-93, `docs/GAIN_STRUCTURE.md`) is the last thing
every block passes through, live or exported. A NaN or infinite sample from a
device that blew up leaves as silence and is counted as a latched fault
instead of reaching the speakers, and a zero-latency safety limiter holds the
output at 0 dBFS. It is bit-transparent to a mix that stays under 0 dBFS.
The master's meter reads the mix *before* the guard, so a mix that is over
still lights the clip latch, and a non-finite sample reads as an infinite
peak rather than as silence.

**The master bus compressor** (MOO-13, `docs/GAIN_STRUCTURE.md`) is the
master's own section, drawn in the master's device rack between its inserts
and its fader, where it runs. Three voicings, each a measured unit's law:
**Grip** (the SSL G bus), **Punch** (the API-2500) and **Tube** (the
Fairchild 670). Grip and Punch show ratio, attack and release, each a switch
reading the unit's own markings; Tube shows the 670's six-position TIME
instead. Threshold, makeup and wet/dry are shared, and each voicing keeps its
own settings when another is picked. A needle meter reads its gain reduction,
with a held mark. Out, it leaves the mix bit for bit. It is saved with the
master's strip.

**The safety limiter's lookahead** (MOO-169) is a small knob on the master's
Out face, beside its clip lamp: 0 ms by default, which is the zero-latency
limiter exactly, up to 5 ms. Above 0 everything leaving the master is that
much later; a take from the hardware input waits for it, and an export trims
it, so a file still starts on the bar line.

**Inside the graph** (MOO-176), the effect host checks each device's input in
the peak fold it already takes for the meters. A block carrying a NaN or an
infinity has those samples silenced before the device sees them, and it's
counted (`EngineHandle::effect_faults`). The status bar raises a warning the
first time that happens. The reverb, plate, modulation effect, gate,
compressor, limiter and Buffer also clear their own state if a non-finite
value gets in, one pass over the block, the way MOO-174 made the shared
filters do. So a device that blows up costs one block of silence downstream,
where it used to silence every device after it until the song was reloaded.

The engine preallocates channel strips, pattern storage, event lists, and audio
buses. A driver-independent render state owns transport, scheduling,
instruments, effects, mixing, and metering. One executor drains fixed-size
commands into that state and publishes position and master peak events, and a
driver adapter hands it buffers: JACK on Linux, Core Audio through cpal on
macOS, chosen at compile time -- or, chosen at run time when that driver will
not open, none. Offline export drives the same render path with no driver at
all.

**With no audio device the app still opens** (MOO-115). When JACK will not
open -- no libjack installed, or no server answering, which the log and the
window tell apart -- the engine runs on a null driver: a thread rendering
512-frame blocks at 48 kHz into nothing, so editing, the transport and the
meters all work and nothing is heard. The window asks "mooloop is running with
no audio" and offers Reconnect. The same question, as "The audio stopped",
appears when a running engine stops being heard: the JACK server shut the
client down (a PipeWire restart does), changed its sample rate, or the
callback has not run for three seconds. Reconnect closes the driver, opens it
again, builds a new engine at whatever rate it now reports, and installs the
open song into it -- samples, routing, mixer -- with the transport stopped and
nothing marked unsaved (MOO-118). Dismissed, the status bar keeps saying so,
and Preferences > Audio > Refresh reconnects. A device that panics in the
audio callback costs a block of silence rather than the audio for the rest of
the session: the block is silenced and counted, the log and the status bar say
so, and the next block renders.

Core Audio has no port graph, so an output target there names a device and two
of its channels. The system default follows whatever the system output is; a
named device that disappears hands playback to the system default, and with
auto-reconnect on, playback returns to the device when it comes back. A new
device or buffer size reopens the stream without rebuilding the engine. The
engine keeps the sample rate the system output had when it started and asks
later devices for the same one.

Under JACK the client asks for the name `mooloop`, and a second instance is
given another (`mooloop-01` by a JACK server, `mooloop-<id>` by
pipewire-jack). Each instance names its own ports from the name it was given,
so two instances each connect, move and reconnect only their own outputs and
MIDI input, and neither offers a mooloop's inputs as an output.

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
construction and sequencer import, on the control thread. The audio callback
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
- DSP tests measure through one shared kit, `mooloop_dsp::testkit`: frequency
  response, alias level, THD, band level and the largest sample-to-sample step,
  with the sample rates every primitive is checked at (44.1, 48, 96 and
  192 kHz). The SVF is held to its bilinear-transform transfer function, the
  biquad's `magnitude_db` to a measured sine, the oscillators to their pitch,
  a per-wave alias bound and the table sine's THD, `Smoothed` to its time
  constant and to settling, and the oversampler to its passband and alias
  reduction -- each at all four rates. A NaN or infinite cutoff, resonance,
  drive, Q, gain, frequency or smoothing target lands on the edge of its range
  rather than in a filter's or oscillator's state, where it would have made
  every later sample NaN. A NaN or infinite *sample* -- a NaN frame in a
  decoded file, a blown-up upstream device -- is lost, but it no longer stays
  in any SVF, cascade, ladder, biquad or one-pole state, and a delay does not
  feed it back round its loop: the next finite sample is heard (MOO-174).
  Other devices' internal state (reverb, plate, chorus) is not yet covered
  (MOO-176).
- The ML-M1's Ladder and Acid filters put their corner in the same place at
  every sample rate: their stages are cornered by solving the stage's own
  response rather than by the impulse-invariant pole, so one Cutoff is one
  sound at 44.1, 48, 96 and 192 kHz (MOO-116). Below a few kHz at 48 kHz
  nothing moves. `scale::cutoff_hz_from_normalized` is the one cutoff-knob
  law: 20 Hz to 20 kHz whatever the rate, with the filter primitives' own
  `0.45 x sample rate` clamp the only place the rate enters.
- Every filtered instrument (v1 mono and poly, the sampler, ML-M1, ML-P8)
  maps Cutoff through that law, so a knob position is one frequency at every
  rate and matches the face's readout; until 2026-09-23 they mapped it to
  `0.45 x sample rate`, so knob 0.75 was 3.8 kHz at 48 kHz and 10.7 kHz at
  192 while every face read 3.56 kHz (MOO-119). They share one voice-cutoff
  block (`voice_filter::VoiceCutoff`: six octaves of envelope, keytracking
  from middle C) rather than five copies of it (MOO-144). ML-P8's feedback
  DC blocker is specified in Hz.
- Glide slides linearly in pitch and arrives in the Glide time: an octave up
  and an octave down are mirror images, and a 100 ms glide is on the new note
  after 100 ms (MOO-145). It was a one-pole in Hz, lopsided in pitch and about
  4.6 times slower than its label.
- Resonance tapers exponentially on every SVF-based filter (the v1 synths,
  the sampler, ML-P8, the Filter effect, ML-M1's clean model): the peak grows
  by about 2.6 dB for each tenth of the knob up to +20 dB, where it used to
  sit flat for most of the knob and put 9 dB in its last tenth (MOO-123). A
  24 dB slope is one shared compensated cascade (`filter::SvfCascade`, which
  was ML-P8's LP24): its corner lands where the 12 dB one's does and it peaks
  about as hard, so the Filter effect's 24 dB mode no longer reaches +40 dB,
  and that effect bends its output under the voice ceiling (MOO-124). Delay
  feedback saturates in the loop above half scale, so a loud input into 0.98
  feedback settles at twice full scale at most rather than fifty times.
- **A song saved before 2026-09-23 sounds slightly different** after these
  changes, with every stored value unchanged (they are normalized, and the
  curves under them moved): a Cutoff knob at 0.75 is 3.56 kHz where it was
  3.77 kHz at 48 kHz (about a semitone down; more at 96 and 192 kHz, where
  it was far brighter); mid-knob Resonance is less peaky (at 0.5 the SVF's
  damping is 0.45 where it was 1.05) with the same top; a glide arrives in
  its Glide time, about 4.6 times sooner than before; ML-P8's LP24 and the
  Filter effect's 24 dB mode peak harder or softer to match their 12 dB
  modes; and delay repeats above half scale saturate. There is no migration:
  the old curves are what MOO-116, 119, 123, 124 and 145 fixed.
- The v1 mono synth's LFO is one shape (sine, triangle, saw, square, or sample and
  hold) with a depth per destination: pitch, filter cutoff, pulse width, and
  tremolo. It free-runs across notes and silence unless set to retrigger.
- Mono synth amplitude is continuous by construction: the amp envelope attacks
  from its current level rather than restarting at zero, and velocity,
  oscillator levels, cutoff, and drive are one-pole smoothed over 5 ms so
  neither a retrigger nor a knob turn steps the waveform.

## Behaviour, And Where It Stops

> **This section is mislabelled and is being split.** It was called "Important
> Limitations", and `### Mixing, Routing, And Effects` below it runs to 588
> lines that are mostly *features* — "the mixer is a list of tracks", "sends",
> "a channel strip on every track". A reader looking for what does not work has
> to read what does, first. Renamed 2026-09-14 so the heading stops lying; the
> content split is a separate pass, because deciding which of ~700 entries is a
> gap and which is a description means reading all of them, not their headings.
> Until then, treat this as *behaviour plus its edges*, and `SCOPE.md` as the
> list of what is actually missing.

### Event And Voice Model

- Probability, microtiming controls, ties, and parameter locks are not yet
  implemented. Note starts and lengths otherwise retain PPQ precision.
- **A note the sequencer started is always ended**, as of 2026-09-23
  (MOO-99). Each channel keeps a table of the voices its pattern started, and
  every edit that takes a note-off out of the playhead's reach releases the
  voice: deleting, shortening, moving or re-pitching a sounding note, a
  pattern-length change, a placement removed, a playback-mode or pattern
  switch, and a mute or solo. Lengthening a sounding note or changing its
  velocity leaves it ringing. A note that ends while its channel is muted is
  still ended, so unmuting no longer brings back a frozen voice. A Song-mode
  loop fold releases the pattern's voices with a note-off rather than choking
  every channel, so a chord the player is holding rings across the loop point
  and a pad's release tail rings over it. **Panic (All Notes Off)** is a
  bindable action with no default chord: it ends every voice and every held
  key, pedal included, without stopping the song.
- One channel holds at most 1,024 notes in one pattern (the engine's
  preallocated store; `docs/CAPACITY_POLICY.md`). As of 2026-09-23 (MOO-133)
  every way of adding a note -- drawing, painting, a step, a step slice, a
  roll slice, duplicate, paste and recording -- refuses at the cap and the
  status bar says why; a paste or duplicate that would cross it is refused
  whole rather than cut short. Before this the next note was drawn but
  silent, and the song refused to save.
- NoteOn, NoteOff, and choke events are sample-accurate and deterministically
  ordered. One-shot loops exit into their remaining sample tail; gated loops
  release through the amplitude envelope.
- **A forward loop's seam can be crossfaded** (MOO-43, 2026-09-24). Loop
  fade, beside the loop mode on the sampler's header, is 0 to 100 ms of the
  sample's own time. The loop's last stretch blends, equal-power, into the
  material just before its start, so its last frame is its first frame's
  neighbour and the seam has nothing to click on. The loop keeps its length
  and its start (a break's downbeat) plays as it always did. A loop that
  starts at the region's first frame has no material before it, so its end
  fades out to silence and its first millisecond fades back in. The
  waveform shades the span the fade covers. Reverse crosses the same blend, ping-pong has no seam, a fade
  is never more than half the loop, and 0 ms (the default, and every song
  saved before) is the hard seam, bit for bit. The fade is a descriptor, so
  it can be automated and modulated.
- **Fit to tempo says what it's doing, and SYNC off keeps the sound**
  (MOO-39, 2026-09-24). With SYNC on, a line under Bars reads the loop's own
  length and tempo and what it lasts fitted, for example "1.00 s at 120.0
  BPM → 2.00 s". It turns the warning colour when the bar count gives the
  loop a tempo outside 60-200 BPM, or needs more stretch than the sampler
  has, and the status bar then says why and suggests a bar count. The Bars
  field also takes the loop's own tempo, "96 bpm", and turns it into bars.
  Turning SYNC off writes the ratio it was running into the Speed knob, in
  the same undo step, so the loop keeps sounding the same until the knob is
  touched. That ratio is the root key's at the current tune; a transposed
  note plays shorter or longer from then on, which is what a fixed ratio
  means. A fitted loop renders the same offline as live.
- **A loop's bounds can snap to a grid** (MOO-47, 2026-09-24). Loop grid,
  beside the L/R fields, is Free, Slices, or 1 bar down to 1/32. A division
  is of the sample's own bar count (the Bars fit-to-tempo uses), counted
  from the playback region's start. Slices snaps to the slice markers and
  the region's ends. It applies wherever the bounds come from, the markers,
  a lane or a modulator, so a lane sweeping Loop start steps through the
  grid in rhythm instead of sliding through every frame. The loop band is
  drawn at the snapped bounds. A grid can't collapse or invert the loop: a
  loop shorter than one step becomes one step. A grid with fewer than two
  points inside the region leaves the loop free. Loop grid is automatable
  and not modulatable, and Free (the default, and every older song) is the
  loop as it always was.
- **Slices can be detected** (MOO-44, 2026-09-24). DETECT in Slice mode
  finds the hits in the playback region and previews a ghosted marker on
  each. Nothing changes until the preview is accepted. While it's up, the
  slice row holds Sensitivity (how quiet a hit still counts) and Spacing
  (the closest two markers may land, 10-250 ms). Each of them re-detects.
  REPLACE keeps the markers placed or moved by hand and replaces the rest,
  MERGE keeps every marker and adds the detected ones that aren't beside
  one, and CANCEL changes nothing. Either accept is one undo step. The
  detector measures both channels and a high-passed copy of each, so a hit
  panned to one side counts and a steady tone doesn't. Each marker lands
  just before its attack. Detected markers are ordinary markers. Whether
  a marker was placed by hand is saved, and every marker in an older song
  counts as hand-placed.
- **A sliced break becomes a pattern** (MOO-46, 2026-09-24). PATTERN, in
  Slice mode, writes one note per slice into the channel's current
  pattern, at the tick the slice falls on in the break. The break is the
  playback region, `Bars` long. Each note is the base note plus the
  slice's position, the same mapping the keyboard plays, and lasts until
  the next slice. Placement keeps the break's own timing unless 1/16 is
  lit. REPLACE clears the channel's notes in that pattern first, and ADD
  writes beside them. A pattern shorter than the break grows to hold it.
  Either is one undo step. Slices past MIDI 127, or past the longest a
  pattern can be, are left out and counted in the status bar. The notes
  are ordinary pattern data from then on.
- Sampler voice allocation is fixed-capacity and deterministic: restart reuses
  the oldest matching pitch, layer mode overlaps notes, and overflow steals
  a releasing voice before a held one, the oldest of either. The sampler, the
  Poly Synth and the ML-P8 all steal in that order (MOO-110, 2026-09-23).
- **Nothing a voice does ends mid-waveform** (MOO-110, 2026-09-23). A voice
  the sampler steals -- which on the default patch, one voice in Restart, is
  every new note -- moves to one of the sixteen slots above the Voices count
  and fades there over the choke's 5 ms while the new note starts fresh in
  its place. A non-looping region or slice fades over its last 2 ms (at most
  a quarter of a short slice) instead of stopping on whatever sample it
  held. Lowering Voices on the sampler or the Poly Synth, or switching the
  Poly Synth to Mono, fades the voices it retires. `continuity_tests.rs`
  holds a default-patch retrigger and a region end on a sine to the family's
  largest-step bound; before, each stepped by up to the whole signal.

### Recording Into The Sampler

Clip recording, the way Ableton's or Bitwig's clip mode records and Maschine's
sampler does (`docs/plans/archive/audio-recording/`, steps 02-05, 2026-09-18):

- **Every channel has an AUDIO row** in the channel sidebar, beside MIDI IN
  and independent of it: Off, the master, any track, any channel -- itself
  included. It names its source by identity, so it follows moves, and says so
  when the source has been deleted. Any number of channels may hold one, and
  it stays set.
- **The sampler face has a RECORD page**: REC, the take's length as it grows, a
  live waveform, CLIP and LENGTH (1-64 bars), and what it records FROM.
  Pressing REC waits for the next bar line -- starting the transport if it is
  stopped, so a stopped song gets a bar of count-in -- then records the AUDIO
  input sample-exact. It stops when REC is pressed again, the transport stops,
  or, with CLIP on, after LENGTH. Several samplers can record at once.
- **A finished take becomes the sampler's sample**, on the channel that
  recorded it whichever is selected, as one "Record Take" undo step. Nothing
  is written into a pattern; the take is heard through whatever triggers the
  sampler. Takes are written to the shared `recordings/` folder in the data
  directory (`~/.local/share/mooloop/recordings` on Linux; beside the
  settings on macOS), and a
  save copies a take into the song's own `recordings/`, under the name it was
  recorded with, whichever asset mode it uses.
- **A take that has nowhere to land says so and keeps the file.** A take
  outlives what it was armed against: its channel can be deleted while it
  records, and -- a channel being a dumb slot -- its sampler can be swapped
  for another device. Either way the recording stays in `recordings/` and the
  status bar says which happened, rather than the take being written onto a
  channel that cannot show or save it. A take is checkpointed every second,
  so its file on disk is readable up to the last second even while it
  records. A take whose file could not be finished -- a full disk, say -- is
  reported the same way, and what it had checkpointed is kept and lands on
  the channel; one that failed inside its first second leaves no partial
  behind.
- **Takes a crash left unfinished open again** (MOO-75). At startup every
  take in the recordings folder whose WAV header counts less than the file
  holds -- a crash inside a take's first second leaves one saying zero frames,
  which nothing could open -- is patched from the file's length, and the status
  bar says how many. The same startup moves an old
  `~/.config/mooloop/recordings/` into the data directory, once, leaving a link
  behind so songs that name a take by its old path still find it.
- **REC refuses a disk without room for a minute of audio**, and says how
  much the disk has left, instead of starting a take that would be cut short.
- **REC refuses a source that is gone, and says which kind.** A channel or
  track that has since been deleted sends you back to the AUDIO row; no audio
  input device at all -- unplugged, changed, or a microphone permission macOS
  refused -- sends you to the audio preferences. Either way REC says so
  instead of recording digital silence and making it the sampler's sample. It
  is the same rule the AUDIO row draws the source as missing by.
- **Quitting finishes a take that is still recording, without asking.** The
  recording is written out to `recordings/` complete, rather than left as a
  file whose header says it is empty; what is outstanding is a fraction of a
  second, so there is no dialog. The wait is bounded, and one that runs out
  removes the partial rather than leaving an unreadable file behind. What you
  get is a finished *file*, not a sample in the song -- the song is closing.
- **Quitting also offers to clear up takes nothing used.** Once quitting is
  settled, takes recorded this session that neither the song nor its undo
  history still points at are listed in the app's own takes dialog, ticked,
  with each one's length, size and date and the total it frees: **Trash and
  Quit** moves the ticked ones to the desktop trash rather than deleting them,
  and **Keep and Quit** leaves them. Either way the app quits. A take an undo
  could still reach is never offered, which is why the offer is made at quit:
  closing the project is when history-only takes stop being reachable.
  Cancelling the quit clears up nothing.
- **File > Clean Up Takes** (`recording.clean-up`, MOO-38) opens the same
  dialog at any time, with two lists. *Not used by this song*, ticked: this
  session's takes in the shared folder that nothing reaches, and takes in the
  song's own `recordings/` that neither the open song, its undo history nor
  the song as saved on disk plays. *Left from earlier sessions*, unticked and
  saying why: shared-folder takes older than this run, which a crash (or a
  quit while only the undo history used them) leaves behind and which may be
  the only copy of a take from a song that was never saved. Nothing moves
  until **Move to Trash**, and then only to the trash.
- **The hardware input is an AUDIO source** under both drivers. Under JACK,
  since 2026-09-19, it is "Audio In": `mooloop:in_l`/`in_r` wired to the first
  physical capture pair. Under Core Audio, since 2026-09-20, it is the
  system's default input device, listed by that device's own name -- Core
  Audio opens a device rather than joining a graph, so there is a device to
  name. A take from it starts the round-trip latency after its bar, so it
  lines up with what was played. With it picked, the AUDIO row shows a peak
  meter and **MON**, which plays the input through the channel -- off by
  default, not saved, and never switched on by anything else, because a
  microphone through speakers feeds back.
- **On a Mac the input row appears only if there is an input.** No input
  device, a device that will not run at the engine's sample rate, or a
  microphone macOS has not granted mooloop, and the AUDIO row simply lists no
  input; the reason is logged once at startup. macOS asks for the microphone
  the first time mooloop opens it, and refusing it is not an error the
  interface reports anywhere else.
- **Not yet:** deleting takes nothing uses (step 06).

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
  Every sounding voice is released at the loop point, at a seek, and when the
  current pattern is switched **in Pattern mode** under a running transport,
  because in each case the note-off it was waiting for is no longer on the way.
- **Selecting a pattern is a view change everywhere else, and costs nothing.**
  In Song mode the selection is not what is playing -- it is what the editor
  draws and where a recorded note goes -- so switching it while the song runs
  releases nothing. Nor does switching with the transport stopped, where an
  audition or a held key belongs to the player rather than to the pattern being
  left, nor re-selecting the pattern already current, nor a selection past the
  end of the bank. Before 2026-09-20 all four cut off every sounding voice on
  every channel.
- **Moving around the app does not interrupt what is playing**, as of
  2026-09-20, and that is now a rule rather than a set of fixes. Selecting a
  pattern, a channel, a bus, a device or a track is a view change: it changes
  what is drawn and what the next edit will address, and none of it reaches
  the audio thread. The one gesture that must tell the engine anything is the
  pattern selection, because the active pattern is also where a recorded note
  goes -- and the engine charges for what changed rather than for the fact
  that a command arrived. `scripts/dupe-audit navigation-sends` reports a
  selection handler that breaks the rule, and its clean run is its answer.
- **A seek no longer rings the old position over the new one.** Delay,
  modulation, reverb and plate tails are cleared when the transport is seeked
  or stopped, because what they hold is audio from a part of the song that is
  no longer playing. An unsynced LFO free-runs through a seek. A
  **tempo-synced LFO follows the song position** as of 2026-09-23
  (MOO-127): while the transport runs its phase is re-derived from the
  position in beats every control tick, so Play, Stop-and-play and Seek land
  it where the position implies, and an export -- which builds fresh
  modulators at the top -- hears the phase playback did. Stopped, it
  free-runs; one set to retrigger on notes follows the notes instead. Until
  then this paragraph claimed free-running modulation kept a bounce in step,
  which it did not.
- **Tails survive a loop fold and a pattern switch**, as of 2026-09-22
  (MOO-59). A delay repeat or a reverb tail from the end of a song loop wraps
  into its start, the way a groove box plays a loop; before this every lap
  emptied every delay line, reverb and plate in the project. A Pattern-mode
  pattern switch under a running transport no longer clears them either: it
  was meant not to, and was sending a seek behind its own program change.
- The playhead can be moved with the transport running or stopped, snapped to
  the playlist's own musical snap, and it reaches the end of the *song* --
  including the part of a long clip that overhangs the 64-bar start canvas --
  rather than stopping at the canvas edge. Stop still returns it to the
  start.
- Playlist starts use the shared musical snap while retaining absolute PPQ
  ticks and are bounded to a 64-bar start canvas. **Two clips of one pattern
  may not be placed overlapping, but growing that pattern can make them
  overlap anyway** -- nothing revalidates a length change. Both go on playing,
  which is what layering means here; what a click in the overlap resolves to
  is the **latest-starting** clip, the same rule automation uses for layered
  placements, so the buried one can still be removed. Before 2026-09-14 it
  could not be reached at all. The timeline is horizontally
  zoomable. Global swing delays alternate sixteenth notes from 50% (straight)
  through 75% (strong shuffle), preserving note duration in realtime and
  offline rendering. There is no clip dragging, time-signature model, groove
  template, per-pattern swing override, or per-channel timing offset.

### State And Persistence

- `mooloop_core::Project` is the canonical serializable snapshot shared by the
  UI, realtime engine installation, persistence, and offline renderer. The
  live UI still owns incremental edits and produces snapshots for these paths.
- Songs, kits, and channel presets use the v1 bundle contract documented in
  `PROJECT_FORMAT.md`. Saves are durable: the staged file is synced, read
  back and parsed before one atomic rename puts it in place, and the version
  it replaced is kept as `<name>.bak`. One document operation (save, open,
  new, load, export) runs at a time: a second one, from the menu, a shortcut
  or the browser, is refused with a status message, and Quit or closing the
  window during a save waits for it to finish. A save that finishes after
  another song has been opened does not give that song its path. Embedded and
  referenced asset policies are available per save. **Embedding is one-way**:
  a sample the bundle already owns stays there whatever the box says, because
  the bundle holds the only copy of it and writing a reference would delete
  that copy. A referenced save of an embedded song is refused per sample, in
  the save report's warnings, and the Embed Assets box goes on showing
  embedded — it follows the samples rather than the mode, so it tells the
  truth on reopening. Un-embedding for real would mean choosing a folder to
  copy the bytes out to, and there is no such gesture.
- Channel presets are instrument presets for sampler and generated sources;
  sampler presets may carry a referenced or embedded audio file while synth
  presets contain only inspectable parameter state. They are saved and loaded
  from the channel row above the rack, because they span the generator and the
  channel's own state.
- A *device* preset is saved and loaded from that device's own rail in the
  rack -- the same two buttons on the generator and on every effect row. The
  load button offers only the presets saved for that device's kind, and is
  disabled when there are none. The device's header then names the preset it
  came from, and keeps saying so after its knobs are moved. **Saving one names
  the device only once the write has succeeded** -- a preset that cannot be
  written, because the name is too long for the filesystem or the disk is
  full, raises its dialog and leaves the rack row saying what is actually on
  disk. A label is dropped when the device wearing it goes: changing the
  channel's source, loading a channel preset over it, or opening a song or
  kit, which replaces the whole rack.
- File > New Song (Ctrl+N) starts a fresh starter song, asking first when the
  current one has unsaved changes, as Open Song, Quit and the window's close
  button do. **That question is the app's own dialog, Save / Don't Save /
  Cancel** (MOO-91): Save runs the ordinary save, and goes on to quit, open or
  start the new song only once the save has succeeded -- a failed or cancelled
  save leaves you where you were. Quit and the close button ask it in the same
  words. It used to be a zenity question with no Save button, and with no
  zenity it read as Cancel, so a song with unsaved changes could not be quit
  at all. Replacing a preset of the same name and loading a kit that drops
  channels holding notes are asked in the same dialog, and none of them blocks
  the UI thread. **A file chooser is
  asked of the desktop's file chooser portal first**
  (`org.freedesktop.portal.FileChooser`, which KDE, GNOME and most tiling
  setups provide), then of `zenity`, then of `kdialog`; on macOS it is the
  system's own panels through `osascript`. **When none of them can show one,
  that is not a cancel** (MOO-90): Save, Save As, Open, Export and the kit,
  channel and bundle pickers raise the error dialog, listing what was tried
  and what to install, and Load Sample and Add Folder say so in the status
  bar. It used to read as Cancel, so on a desktop without zenity those
  commands did nothing and said nothing.
- Missing samples are recoverable by loading a replacement audio file, but
  there is no dedicated path-search/relink dialog yet.
- **Unsaved changes survive a crash** (MOO-103). Once a minute, while the
  song has unsaved changes and no knob or drag is held, it is autosaved to
  `~/.local/state/mooloop/autosave/` (`$XDG_STATE_HOME`; beside the settings
  on a Mac). Samples are referenced where they are, never copied, and File >
  Clean Up Takes holds back a take an autosave still plays. The next launch
  after a crash, a kill, a power cut or a quit by signal asks "Recover
  unsaved changes to <song>?" (Recover or Discard, and how long ago it was
  written). Recover opens it unsaved, under the song's own file, so Save
  writes there. Saving, or answering the unsaved-changes question with
  Don't Save, removes the autosave. Each running mooloop holds a lock on its
  own autosave folder, so a second window never offers the first one's song.
- **A file operation that crashes no longer locks the File menu** (MOO-103).
  A panic in an open, save, load or export worker (a sample decoder, say)
  is reported in the error dialog as an internal error, and the menu works
  again. It used to stay greyed out until restart.
- **Every run leaves a trace.** The diagnostic log is written on every run to
  `~/.local/state/mooloop/mooloop.log` (`$XDG_STATE_HOME`; `~/Library/Logs/mooloop/`
  on a Mac), rolled aside past 4 MB, and Preferences > Developer shows where;
  it is no longer a preference. A panic leaves a crash report with a
  backtrace in `crashes/` beside it. SIGTERM, SIGINT and SIGHUP -- a logout,
  `kill`, Ctrl+C -- quit through the quit path without its dialog: the log
  says which signal, a take still recording is finished, and unsaved song
  changes are not saved but are kept in the autosave for the next launch to
  offer. `docs/OPERATIONS.md`,
  "Diagnostic Log", has the details.
- **A song old enough to reference the built-in kick opens with it audible.**
  Projects saved before the sampler stopped auto-loading a kick carry a
  `SampleReference::Builtin`, which the install substitutes the cached default
  for. Until 2026-09-15 a second publication four lines later cleared it, so
  the channel was silent while its name, waveform and duration all described a
  kick; `control-plane-seams/03` fixed it.
- **Two sample loads into one channel resolve in the order they were asked
  for, not the order they finish.** Loading a long file and then changing your
  mind used to be decided by decode time, so the first choice could land last
  and win. Each dispatch carries a request token now and a superseded
  completion is discarded.

### Mixing, Routing, And Effects

- Channel solo, mute, volume, and pan are exposed in the rack row, alongside
  the mixer track the channel feeds: volume and pan as compact knobs, solo and
  mute as one 18px chip split across its middle — yellow above, red below.
  Neither half wears a letter; the colour of the lit half is what says which
  is on.
- **Every mixer move ramps** (MOO-107, 2026-09-22). A channel's or a track's
  fader, pan or balance, its mute, a solo silencing it or giving it back, and
  a track's polarity all reach the audio through a one-pole lag of 5 ms
  (`STRIP_GAIN_SMOOTH_S` in `engine/src/render.rs`), the one sends already
  used, per sample -- including when a lane or a modulator drives the fader,
  whose control-rate staircase the lag rounds off. A mute is a fade: the
  channel or track goes on rendering, its output and its sends aimed at
  silence, and stops contributing only once both have arrived, about a
  hundred milliseconds later. Polarity crossfades through zero. A document
  arriving starts at its own values rather than ramping into them, so a
  bounce's first milliseconds are at the levels the song holds.
  `continuity_tests.rs` holds each of these moves on a sustained sine to the
  largest-step bound in `render_test_support.rs`; before the change each one
  stepped by up to the whole signal, and a polarity flip by twice it.
- **Solo in place, per channel**, since 2026-09-22. A soloed channel silences
  the *other* channels and is heard through its own volume, pan, mute and
  track, exactly as a soloed track is. It is the same ruling one level down
  and a simpler derivation: channels do not feed each other, so there are no
  ancestors to keep audible the way a soloed track's feeders are kept. An Aux
  In reading a silenced channel's outlet still hears it, because a producer
  publishes whether or not it is heard.

  A channel's `solo` is what is stored; what it silences is derived from the
  whole bank every pump tick and never written down. A solo does not touch
  the soloed channel's own mute, and dropping a solo gives every channel back
  whatever its mute said. The name of a channel a solo is silencing dims, the
  way a silenced mixer strip's name does.

  A channel's solo and a track's are separate controls over separate banks:
  soloing a channel says nothing about tracks, and soloing the track a channel
  feeds says nothing about its siblings on that track. `channel.solo` is in
  the shortcut registry with no default chord (`docs/ACTIONS.md`).
- **The mixer is a list of tracks, and a track is made because somebody made
  it.** A new song opens with the master; the starter kit adds `Drums` and
  `Bass`, with its four drum channels grouped onto the first. `+` in the mixer
  adds a track, and a track's device face renames it or removes it. Both are
  undoable, and a rename is one undo step however many characters it took.
  Removing one falls anything routed to it back to the master rather
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

  Send levels are **smoothed**, per sample, over the same 5 ms every other
  strip-level gain uses (see *Every mixer move ramps*). A send that appears
  mid-song fades in from silence; one that survives a routing rebuild keeps
  the level it had.
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
  been told about. A short stored bank is a small mixer and is left
  as it is -- padding it back to seventeen was removed. Invalid individual
  routes are repaired to the master by the integrity pass, which reports the
  repair; a send naming a track that is gone is dropped by `sanitize_bank`
  after the load, which reports nothing (`docs/LOOSE_ENDS.md`).
- A send orders its target after its source, the same way an output does, and
  a cycle closed through a send is refused the same way one closed through an
  output is.
- **A channel strip on every track.** Four sections -- an input stage
  (`pre in` and a drive), a four-band EQ, a compressor, and a polarity
  switch -- under one strip-wide **voicing**: `Moo`, `Grip`, `Punch`, `Iron`.
  Every section is **out by default**, and out is not "flat": a section that
  is out does not touch the samples, so a project that has never opened a
  strip renders bit-identically to one built before the strip existed. That is
  what entitles it to exist on every track rather than being a device
  somebody places; the price of having one everywhere is three booleans a
  block, against a track bank that is capped at seventeen today.

  The EQ's four bands read **left to right, top to bottom**: high shelf, high
  mid, low mid, low shelf. Each is gain / frequency / Q, and each switches
  between a bell and the shelf it is nearest -- the top two to a high shelf,
  the bottom two to a low shelf. A band's Q knob is its Q as a bell and its
  **slope** as a shelf, and a band at exactly 0 dB is not run at all. The
  compressor is its own design rather than the compressor device behind a
  different face: threshold, ratio, attack, release, knee, a parallel `w/d
  mix` (at 0 it is the dry signal exactly) and makeup, with a lamp beside the
  section's header that lights with the gain reduction the block actually
  applied.

  **Frequency is stepped, and the step is what is stored.** A band offers 5,
  7, 7 or 5 positions rather than a sweep, and no position is printed with a
  hertz value, because the hertz is the *voicing's*: `StripEqTable` gives each
  voicing its own list, so Iron's low mid can sit lower than Moo's without a
  label going wrong on three faces out of four. The tooltip and the status bar
  say what the current position is worth. Dragging a point on the rack row's
  response plot snaps to the nearest position by log distance.

  **A voicing selects laws, never values.** It owns the input stage's
  harmonic profile, its tilt and its slew limit (all measured -- see
  `docs/plans/archive/console/06-preamp-modelling.md`), the EQ's Q law, the
  compressor's curve above the knee, and its programme dependence. Nothing a
  voicing does moves a number a knob shows, so the 3 kHz on the face is the
  frequency being boosted whichever voicing is selected; what a voicing
  changes that a knob cannot show is *drawn*, by the response plot and the
  gain-computer curve. `Moo` is the null case exactly: every section in, `Moo`
  selected and nothing set is the same audio as no strip.

  The strip is drawn in two places, and no parameter is reachable from only
  one of them: the mixer strip's own face, and a **pinned 2U row in the
  track's device rack**. Where that row sits in the chain -- before the track's own
  devices -- is one statement, `mooloop_core::mixer::STRIP_PIN`, which the
  engine's block loop reads as well, so the drawing and the audio cannot
  disagree. The row has no insert or remove rails, because it can be neither.

  Polarity is drawn beside mute on both faces and acts at the **top** of the
  track's block, so everything after it -- the strip, the chain, both send
  taps and the fader -- sees the flipped signal.

  Not yet: the strip's own processing -- its drive, EQ and compressor -- is
  not an automation or modulation destination (its fader and pan are), and
  there is no strip preset. `docs/plans/archive/console/00-status.md` says why
  each is separable.
- **Solo in place, per track.** A soloed track silences the *other* tracks,
  and the exceptions are what make it useful: anything that feeds a soloed
  track and anything a soloed track feeds stay audible, followed through
  outputs **and** sends, in both directions -- so soloing a group hears the
  group, and soloing a channel's track hears it through the group it lands in
  rather than in isolation from its own destination. Two solos are both heard.
  Nothing is silenced when nothing is soloed, and the master refuses the
  gesture, because soloing the thing everything reaches means silencing
  nothing.

  It is **solo in place**, not a monitor tap: the silence happens where mute
  happens, at the track's own output, so the mix a solo produces is the mix
  minus everything else rather than a separate path with its own gain
  structure. What is silenced is derived from the whole bank every pump tick
  and diffed like compensation, the audio graph and the console sums -- a
  track's `solo` is what is stored, never the silence -- so adding a send can
  change what a standing solo lets through without anyone pressing anything.
  A solo does not touch the soloed track's own mute: a muted track that is
  soloed stays muted, which is the question "is this the one that is quiet?"
  answered honestly. The button is drawn beside mute in the column that
  survives the turn, and on the strip's rack row.
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
  where a cyclic file gives up **the edges that close the loop** so it still
  opens and plays. A send on the ring goes before an output on it, because a
  dropped send loses what was added where a re-pointed output still carries
  the track's audio; routing elsewhere in the bank is untouched, and each
  removal is named in the log. Feedback routing would mean reading a bus's
  previous block, which is a deliberate feature rather than a fallback and
  needs a latency story this engine does not have.
- A muted bus still processes, so effect tails on it decay rather than freeze,
  but contributes no audio and meters as silent.
- Per-bus peaks reach the GUI through a shared array of atomics rather than the
  event ring, which the ring's drain rate could not keep up with. The published
  value is a peak hold that only the GUI's read clears, so a transient landing
  between two UI frames is still shown. The channel rack has no meter of its
  own: `ChannelMeter` is drawn on the mixer strip, the device rack's two rails
  and a track's fader row, and nowhere else. **All of them are continuous
  bars with a peak-hold hairline**; the LED-segment form they had until
  2026-09-15 survives only in the mockup catalog. Preferences > Appearance >
  Metering tunes how fast they fall, from 30 dB/s down to 3, defaulting to the
  IEC rate of about 12.

  **Only a meter with a clip latch behind it draws a clip lamp.**
  `ChannelMeter` takes `show-clip`, and the rack's two rails set it false:
  they meter a chain, and a chain has no latch to light or to clear. A track's
  fader row does have one -- the same latch its mixer strip shows, cleared
  from whichever of the two the user clicks -- and shows peak hold from the
  same reading. **The master's toolbar meter is the same latch again**: it
  reads bus 0 through the mixer strip's own ballistics, so either lamp clears
  both and neither can disagree with the other. Before 2026-09-14 it read a
  separate per-block engine event instead, which is a bounded ring the audio
  thread drops from under pressure -- the always-visible meter was the lossy
  one. Until 2026-09-14 the rails drew a lamp that could not light
  and the fader row drew one that was never bound, while its peak marker was
  pinned to the level.
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
- Thirteen effect kinds ship: a low-pass/high-pass filter, a drive/saturation
  with four curves at 2x oversampling whose Drive changes character rather
  than level -- a signal at the -12 dBFS operating level keeps its peak at
  any drive on any curve, and a hotter one is held down toward it -- a preamp carrying the channel strip's
  four voicings -- Moo, Grip, Punch and Iron, the last three measured from
  real units rather than picked -- over a Drive, Mix and Output in dB. `Moo`
  is the default and is bit-identical to no device at all, which makes the
  preamp the way to put an automatable gain stage in the middle of a chain;
  it is deliberately not oversampled, so it adds no latency and can sit
  anywhere. A bitcrush that is deliberately not oversampled either, a stereo delay with damped cross-feedable feedback and
  digital/tape/reverse responses to a moving delay time. Its Time control is
  a knob with a sync lamp: dark, it sweeps free milliseconds; lit, it steps
  the same twenty-one-entry musical grid the modulators use, `4/1` down to
  `1/64T` with dotted and triplet entries throughout. While synced, every
  project BPM change immediately recalculates and sends the ordinary ms
  parameter to the audio engine, clamped to the two seconds the delay line
  can serve -- a division asking for longer reads in amber. It persists the
  division, not just its current ms result. It is joined by a gate,
  compressor, and limiter sharing one detector and gain-computer module; a
  seven-band parametric EQ with optional bounded spectrum telemetry, **whose
  every band and both pass filters carry their own stable parameter ids** --
  so an automation lane on band 3's frequency means band 3 forever, whatever
  the face happens to be showing, and the band selector is a view control
  rather than an automatable parameter that decided what every other EQ lane
  meant. Its bands run the same two laws the channel strip does, from the same
  function: **a band's Q is its slope while that band is a shelf** and its Q
  while it is a bell, and a proportional bell narrows as it is pushed. The
  shelf half of that arrived 2026-09-14 -- before it, a shelf's Q knob moved
  nothing. **Its response plot draws the filter that is running**, not a
  shape resembling it: Rust designs the same coefficients the audio path
  designs and evaluates their magnitude response, so the drawn curve is
  within a tenth of a decibel of what a sine measures through the bank. Both
  pass filters are on that curve at their real slopes with grabbable corners,
  a shelf's drawn slope follows its Q knob, and the same plot draws the
  channel strip's four bands the same way. The band buttons read 1 to 7 --
  the numbering their parameters use -- and the pass-slope buttons read
  12/24/36/48/72 dB per octave, which is what the bank rolls off at; they
  read LOW/1..6 and 6/12/18/24/36 before 2026-09-15. **All seven bands start
  on and spread across the band** at the seven-band graphic EQ's own centres
  -- 63, 160, 400, 1k, 2.5k, 6.3k and 16k -- with a **low shelf at band 1 and
  a high shelf at band 7**; before 2026-09-15 three were on, the high shelf
  was band 3, and the other four sat on top of each other at 1 kHz, so four
  of the seven handles were underneath band 2's. Every band rests flat, and a
  bell at 0 dB is the identity filter, so a fresh EQ still passes the signal
  through untouched. The target row is drawn in the order the plot reads --
  the high-pass, the seven bands, then the low-pass -- and **a target that
  has a shape of its own draws it**: the two shelves and the two pass filters
  are line-art glyphs, a plain bell is its number, and the glyph follows the
  band's live kind rather than a fixed picture of the opening arrangement.
  The analyzer's switch sits in the plot's own top corner instead of a third
  button in that row, and the selected target's ON sits beside the three
  knobs it switches on. Double-clicking a knob
  returns to the *selected* band's resting value rather than to band 2's. A
  feedback-delay-network hall reverb; and one five-mode modulation processor
  (chorus, flange, phaser, ensemble, and ADT) whose Rate carries the same
  sync lamp the delay does, over the same grid, clamped to the 12 Hz its LFO
  runs to. Its delay-based modes share a
  bounded fractional stereo ring; Phaser uses a stereo all-pass cascade. The
  generic host supplies their dry/wet blend, so the DSP returns the processed
  signal only. The reverb runs eight modulated delay lines through a Hadamard
  feedback matrix behind a diffused, pre-delayed input, each line's return
  smeared through its own allpass diffuser inside the feedback loop so the
  tail blooms into a dense wash rather than ringing on eight sparse modes,
  at a fixed per-sample
  cost independent of decay time and with no reported latency; Size, Decay,
  Damp, Pre, Diffuse, Width and Mod are all ordinary event-driven parameters,
  so every one of them is a working modulation destination. It replaced a
  generated-room convolution player whose per-block cost spiked over a
  64-frame budget at a two-second tail and which could not accept a parameter
  change at all without an off-thread IR rebuild. Beside it is a cheaper
  plate: eight parallel Freeverb-tuned combs into four series allpasses per
  channel, with Size, Decay, Damp, and Width, for material that does not need
  the hall. The thirteenth kind is the retained-audio Buffer described below,
  which is an ordinary insert in the same picker. Device faces are
  width-quantized in rack units: filter, drive, preamp, bitcrush, limiter,
  and plate take 1U; gate, compressor, EQ, Mod, and Buffer take 2U; delay and
  reverb take 3U.
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
  gain to both, so compression cannot walk the stereo image around.
  **The limiter looks ahead and limits true peaks** (MOO-142, 2026-09-23).
  It holds its audio back 96 frames (2 ms at 48 kHz) and declares that
  latency, so the mixer compensates it like any other -- which means **every
  existing Limiter now adds 96 frames of latency** to its channel, and every
  other channel is delayed to meet it. Its gain computer
  finds each frame's 4x-interpolated true peak, holds the deepest need across
  the lookahead, and ramps into it so the gain has arrived when the peak comes
  out. Nothing leaves above the ceiling, between samples included, without the
  hard clamp that used to do the real work on every transient; the clamp is
  now only a backstop. It sounds cleaner on transients: they come down
  smoothly instead of being squared off, and there's no clipping distortion.
  **The gate has two thresholds**: it opens at the knob and shuts only once
  the level falls 6 dB under it (`GATE_HYSTERESIS_DB`), judged on a level
  detector that holds across a waveform's troughs, so material sitting on
  the threshold no longer flicks it open and shut. It sounds steadier on
  sustained material near the line, and a sparse hit rings a few
  milliseconds longer before the gate shuts. **The compressor has its own
  Mix**, a linear parallel balance like the channel strip's `w/d mix` (0 is
  the input exactly), where the header's equal-power Wet ran a compressor and
  its own input 3 dB hot at 50%. Its gain computer was already the strip's
  under the `Moo` voicing (the same detector and curve); the voicings'
  programme dependence and ratio bend stay the strip's own.
- Each kind publishes a static `ParamDescriptor` table
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
  would, and is designed to nest four deep -- though nothing refuses a fifth,
  and a box past the cap stops blending (it still bypasses; see
  `docs/LOOSE_ENDS.md`). It is made either from the insert menu, like any
  other device, or by wrapping a device that is already there (the fourth
  button on its left rail). Inserting from a container's own `+` puts the new
  device *inside* it; inserting from a leaf's `+` puts it before that leaf.
  Appending to the end of a box is a drag rather than an insert, because the
  end of a run has to stay addressable as "after the container" -- for an
  empty box, "just inside" and "just after" are the same position, so which
  one is meant has to come from the gesture rather than from the index. Its one control is a dry/wet mix across the
  whole run, delayed to match that run's latency — the wet/dry that a
  *single* device has always had, applied to a group. It really is the one
  control: the shell's own dry/wet is not offered on a container row, because
  a box has no node to be wet with. Bypassing a container skips its run
  without moving the channel in time. **Nothing in the
  interface**: a device's left rail wraps it in a container, a container's
  right rail unwraps it, and dragging a device onto a row already inside a box
  puts it in that box. **Containers nest four deep and the wrap button goes
  out at the fourth**, because the engine preallocates one dry buffer per open
  box and a fifth one would have an inert Mix and no chrome. A *device* inside
  the fourth box is fine: the cap counts boxes, not rows. And **dropping onto
  an emptied box puts it back
  inside** -- an empty container's span covers no index, so its own row is
  the only thing there is to aim at and a drop on it means "into this".
  Dropping on a container that still holds something keeps meaning "before
  it". **The run is drawn as a box, and the box is the container's own
  chrome**: its input rail stands at the head, its *output* rail stands past
  the last device it holds and meters what leaves the run rather than what
  entered it, and the recessed space between them is what its devices sit
  in. Both rails and that space are one colour, darker than a
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
  `docs/plans/archive/containers/00-status.md`.
- A second container kind, **Layer**, sits under Chain in the insert menu and
  saves and reloads as its own kind. It holds a run, nests, wraps, bypasses
  and mixes as a chain does, but **it splits its input across its direct
  children and sums them**: each device it holds directly -- or each Chain
  it holds, with whatever that chain holds -- is one parallel branch fed the
  layer's input. Branches sum at unity (two identical branches are exactly
  6 dB up), a branch holding a Drive is matched by delaying the others to it
  so the sum does not comb, and the layer declares its longest branch as its
  latency. Its Mix blends the sum against its input; bypassing it passes the
  input, delayed by that latency. A layer of one branch is a chain.
- **A layer draws like Bitwig's FX Layer** (`containers/09`). Its face is one
  rack unit: a list of its branches, each row with a name, **S**, **M** and a
  level meter, a `+` under the list that adds an empty branch at the end, and
  **Gain** and **Mix** knobs. Only the selected branch's devices are drawn in
  the rack, to the right of the layer under an accent bracket; clicking
  another row in the list shows that branch instead. Which branch is shown is
  not saved and is not an undo step. Every branch the `+` makes is a Chain,
  whose face now has a **Level** knob beside its Mix: inside a layer it is
  the branch's fader. Mute takes a branch out of the sum and Solo keeps only
  the soloed branches of that one layer; both ramp, and both are undoable
  and saved. A device sitting directly inside a layer, not in a chain, is a
  branch with no S or M.
- **Making and emptying a layer** (`containers/10`). The rail's wrap button
  opens a menu, **Chain** or **Layer**. Wrapping in a layer makes a layer of
  one branch, a Chain holding what was wrapped, so the branch has its S, M
  and Level from the start. A right-click on a branch in the list offers
  **Remove branch**, which takes the branch and everything in it. The last
  branch can go too, leaving an empty layer that passes its input. Each of
  these is one undo step.
- **A layer saves as a preset**, the box and every branch in it, and lists
  on the layer's own preset rail. It ships with a bank of three: Parallel
  Drum Compression, Clean and Distorted, and Three-Way Split (a low-pass, a
  band-pass and a high-pass on three branches).
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
  module's `input_slot` across the same permutation. **A reorder also moves
  each module's running state**: an LFO keeps its phase, its smoothing and its
  fade position, an envelope keeps its stage and level, and a Random module
  keeps its sequence. Before 2026-09-14 a drag rebuilt every moved module, so
  an envelope dragged to the front dropped a held note's contour to zero
  mid-sustain, and two LFOs dragged past each other swapped params without
  swapping phase and both jumped. Arming a module's Assign
  switch makes legal controls assignable; dragging one sets route depth while
  the control keeps its base value. Removing a route restores the
  destination's base, on generator parameters as well as effect ones. The
  envelope's gate input is an explicit channel-note picker — the first
  adapter for a typed generator `Gate` outlet, which does not exist yet.
  A published generator outlet is a source in the same shelf, in its own pane
  beside the modules. Device (effect) outlets, cross-channel sources, and
  macros remain planned.
- **The retained-audio Buffer is a ring that is always recording, and four
  ways to hear it instead of the input.** Rebuilt 2026-09-16, the day it was
  first played; what it replaced was a turntable model, described at the end
  of this entry.
  **Three held gestures, each owning its own settings.** JUMP plays forward
  from `Jump Back` behind now; REVERSE plays backward from now; STUTTER
  repeats the last `Stutter` length. Each holds the ring still while it is
  down — the press is a freeze — and letting go returns to live audio and
  restarts the writer. JUMP and REVERSE wrap round the ring rather than
  running out, so a held button never lets go on its own. None of them
  borrows another's setting: the stutter's length is its own knob, which is
  the question the rebuild started from. The three are **gates** rather than
  triggers — `Jump`, `Reverse` and `Stutter Gate` are high for exactly as long
  as the gesture lasts — so a finger on the button, a held MIDI note and a
  block drawn in a lane are one mechanism. Last one pressed wins.
  **`Position` is a playhead, heard only while it moves.** A static Position is
  a setting nobody is playing, and the device falls through to live audio.
  The head *is* the position rather than chasing it, so the playback speed is
  the position's own speed: a one-bar saw from the modulator rack over a
  one-bar ring plays at unity, half the period is an octave up, and a
  descending ramp is reverse. Nothing on the face names a rate. A move too
  large to sweep (past four times speed — a saw's wrap) cuts under the
  crossfade instead of zipping. At the default full `Span` it addresses **the
  ring in the ring's own coordinates**, the same map the waveform is drawn in,
  so dragging across the picture scrubs exactly what is under the pointer; a
  shortened `Span` is the most recent that much and follows the writer.
  **Frozen with nothing else playing, the ring plays** — forward, round and
  round — so a freeze leaves a loop rather than a still. Priority is fixed and
  short: a held gesture, then a moving playhead, then a frozen ring, then the
  input. Every change of source crossfades by `Crossfade`.
  **`Quantize` delays presses, never releases.** A gesture and a freeze wait
  for the next `Quant Start` boundary (one bar by default); letting go before
  it lands takes the press back, and the face shows ARMED meanwhile. `Quant
  Start` is independent of every length on the device — starting on the
  quarter while stuttering a thirty-second is the ordinary case. With the
  transport stopped there is no grid, so a press lands at once. A saved
  freeze is restored rather than quantized. A waiting freeze and a waiting
  gesture are independent — both land, the freeze first, so the gesture
  plays over the frozen ring — and a gesture still waiting outranks nothing
  until it lands. The wait is measured from the press's own frame, and a
  setting written on the same tick as a press (`Quant Start`, a length)
  applies before it.
  **Every length is on the shared musical grid** — the twenty-one
  `ModTimeDivision` entries the modulator racks and the delay use — and `Span`
  adds one position past them for the whole ring, which is its default.
  **A fresh Buffer keeps two bars of history**, adjustable from 1 to 64 on the
  face's HISTORY stepper. `bars` is not a descriptor parameter and cannot
  become one: resizing reallocates, so the replacement is built on the control
  thread and swapped in at a block boundary, down the road a tempo change
  travels. **The replacement takes over the history** (MOO-137): the most
  recent frames the old ring held, as many as the new one has room for, so a
  tempo change or a HISTORY change no longer empties an unfrozen Buffer. A
  frozen buffer refuses that swap rather than losing what is playing, which
  also means **changing HISTORY while frozen does nothing to the running
  ring** until it thaws and something resizes it again. An undo, a redo or
  any other whole-project install keeps a Buffer whose own settings did not
  change, with its ring, and keeps the channel it sits on sounding.
  `Freeze` persists, but the frozen audio itself is not saved (MOO-196 asks
  whether it should be). So a Buffer that arrives with Freeze on and an empty
  ring -- a reopened song, a preset, a paste -- records, with the freeze
  armed (the face shows ARMED), and the freeze lands once the ring holds a
  full history. It used to latch the empty ring and play silence under a
  face reading FROZEN. A document saved with a gesture held reopens holding
  it.
  The face's SEAMS readout counts wraps and cuts — a stutter's repeats, a
  reverse head lapping the ring — which is the number that says whether the
  head is doing what the picture claims. It replaced RETURNS, a count of
  forced returns to live, which is a failure this design cannot have.
  **What it replaced, and what that leaves behind.** The turntable model had
  one read head fought over by `Position` (which armed a *chase* whose closing
  speed was the playback rate), `Rate`, and a `Length`/`Loop` window, under
  an arbitration rule with arrival and stillness tests; the face's buttons
  were macros writing those shared knobs, so STUT could not have a length of
  its own. `Rate`, `Length` and `Loop` are retired, and ids 3, 4 and 5 are
  spent alongside `Offset`'s 0. Their saved values are dropped on load, and
  **an automation lane or modulation route that names one of them stays in
  the document but drives nothing**. `Quant Grid` is now `Quant Start` on the
  same id, and a saved `quant_grid` key loads into it. A project saved before
  either change still opens with its `Offset` converted to `Position`, lanes
  and routes included.
  The `BufferMidiMap` event path still builds its own head from a
  `BufferEvent`'s geometry, and is the one path that can ask for a speed other
  than ±1; it still has no caller outside its own tests.
- **Every generator is descriptor-addressed** through `GeneratorParams`, so
  their parameters automate and modulate like an effect's. The
  three-oscillator synths reserve ten parameter ids per oscillator, starting
  at 100; ML-P8's, DS-01's and the v1 drum synth's ids are each their own
  namespace starting at zero, because none of them is that voice with a
  different count. `docs/MODULATION.md` records the approved design;
  build order is in
  `docs/plans/archive/buffer-implementation/02-control-and-modulation.md`.
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
  polarity is about the module convention.** A rack module emits into
  `-1..1`, and `Unipolar` lifts that into `0..1` so a one-way module rests at
  the destination's base. The lift stands on the module's **own amount**, not
  on the full range: an LFO at half depth swings `-0.5..0.5`, and a unipolar
  route from it still rests on the base and reaches half the route's depth.
  Turning that amount to zero therefore contributes nothing, where until
  2026-09-14 it parked the destination half a depth above the base while the
  module visibly stopped moving. An LFO still fading in is the one case the
  lift cannot see, because the fade is engine state rather than a parameter:
  for the length of the fade a unipolar route from it rises from half the
  module's depth instead of from the floor. An outlet publishes in its
  *declared* range, where a
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
  one channel, one outlet. **Parallel sends are no longer absent**, but they are
  a second edge system rather than this one: a send is a producer-side edge that
  carries its own compensation, where an aux-in subscription lands pre-chain in
  the consumer and has nowhere to put a delay, which is why it is refused when
  its tap is late instead. Unifying the two is recorded and not done. Sidechain
  key inputs are still absent.
- The ML-P8 has a device output stage: Volume and Pan, before the channel
  strip's own. They exist to be the base its per-voice `VcaLevel` and `Pan`
  modulation destinations offset from, which resolved from hardcoded unity and
  centre before them -- so a Velocity route on Pan now swings around wherever
  the patch put the device, and Spread widens around that rather than around
  the middle. Volume is one-pole smoothed over 5 ms like the device's other
  levels, so modulating it (a kick-gated envelope pumping a pad) or dragging
  it glides instead of stepping once per 32-frame control tick (MOO-214).
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
- **A lane that stops driving a destination gives the knob back.** Deleting or
  clearing a lane does it, and so does moving the playhead off it: switching
  pattern, switching between pattern and song mode, and seeking all hand every
  destination the outgoing position was driving — and the incoming one is not
  — back to the value its knob shows. Without that a filter goes on playing at
  wherever the last curve left it while its face reads something else.
  **A song-mode clip boundary is the case this does not cover**, because it is
  not a command: the playhead moves out from under a lane on its own, and the
  destination latches until something touches it. That one is in
  `docs/LOOSE_ENDS.md`.
- **The mixer is latency compensated.** Every device declares the frames it
  adds, the bus tree compiles into a per-producer delay, and each channel and
  bus waits by the difference before it sums — so two channels hitting on the
  same tick land in the same frame even when one carries an oversampled device
  and the other does not. Only Drive costs anything today (fifteen frames), so
  the audible effect is small; what it removes is the comb filtering that was
  worst exactly when two channels were most alike, and what it unblocked is
  parallel sends — now built, and compensated per edge rather than per producer,
  because a track with a send reaches two summing points that owe it different
  delays — and sidechains, which are untrustworthy without it and are still
  absent.
  Bypass keeps its device's latency — a bypassed node's signal goes through
  the same delay rather than past it — so A/B-ing an effect A/Bs the effect
  and not the timing. Removing the device is what gives the latency back. The
  plan is derived from the project rather than tracked alongside it, so no
  edit path can forget to update it, and an offline render compiles the same
  plan as a live one.
- **A channel** feeds exactly one track and cannot author a send of its own.
  The engine's sends are strip-level and a channel's compiles correctly, but
  nothing authors one, because the mixer draws no channel strips for the control
  to live on — that and the tap points below pre-fader are stage 2 of the send
  work. A send has a level and a tap, and no pan and no wet/dry split of its
  own. There are no sidechains, external inputs, or per-track stem export.
- Latency compensation covers every device, native or hosted. `AudioNode`
  reports integer processing latency and `EffectKind` declares it without
  being built; the drive is the only kind that costs anything, at the measured
  15 frames of its complete 2x interpolate/decimate path, and it also delays
  its internal dry path by the same amount so its own wet/dry control cannot
  mix time-misaligned signals. Channels with unequal effect latency no longer
  comb-filter when they meet at a bus -- see the mixer entry above. A hosted
  plugin's latency is its own, read once it is active: the compensation plan
  asks the plugin for it, live and in an export (MOO-81). A plugin *inside a
  container* is still sized as zero latency there (MOO-212).
- **A CLAP effect plays in a chain, headless** (MOO-81, plugin-hosting 06).
  **Nothing in the window inserts one yet**: the browser, the insert-menu row
  and the plugin's face are step 08. Today a plugin reaches a song in two
  ways. One is a song that already names it: a `plugin` effect device whose
  slot is in the song's `plugins` table (`PROJECT_FORMAT.md`, "Hosted
  plugins"), opened from disk or the command line. The other is the
  session's `Session::insert_plugin_effect`, which tests and
  `crates/mooloop-session/examples/clap_effect_case.rs` call. mooloop finds the
  plugin by its id in the scanner's cache (`<config>/plugins.toml`, below),
  rereading the cache when a scan rewrites it. It loads the plugin with its
  saved state and activates it at the engine's rate. On the next pump tick it
  swaps the plugin's processor into the device, which played as a
  pass-through until then. Only a plugin with one stereo input and one stereo
  output is hosted. Anything else is refused as incompatible, and so is a
  sidechain. The plugin is heard live, and **it is in an export**: the export
  renders with second instances opened from the live ones' state. A plugin
  that reports an error or a non-finite sample, or panics, is passed through
  from that block on. A song whose plugin is **missing** opens, plays that
  device as a pass-through, and saves it back unchanged, lanes and routes on
  its parameters included. It is tried again when a scan finds new plugins.
  A structural edit (paste, move, delete, undo) keeps a hosted plugin
  running, state and all. A new sample rate, or a restart the plugin asks
  for, rebuilds its processor. **The rebuild fades rather than clicks**
  (MOO-213). The plugin fades out to the dry signal over about 35 ms, the
  device plays dry, still as late as the plugin was, while the new
  processor is built, and then the plugin fades back in. A missing plugin
  that turns up fades in the same way, unless it reports latency: then the
  channel moves later by that much when it arrives, as it does for any
  latency change. On quit, mooloop waits up to two seconds for
  every plugin's processor to come back from the audio thread before it
  destroys the plugin. Its GUI does not open (step 11).
- **A hosted plugin's parameters take lanes and routes** (MOO-82,
  plugin-hosting 07), though nothing in the window draws them until step 08
  gives the plugin a face: a lane or route reaches a plugin parameter today
  from a song that names one, or through the session
  (`crates/mooloop-session/examples/clap_automation_case.rs`). A lane sets
  the parameter at every 32-frame control tick, in an export and live alike;
  a route is an offset over the plugin's own value (CLAP's parameter
  modulation), gone when the route goes. Only a parameter the plugin marks
  automatable takes a new lane, and only a continuous one it marks
  modulatable takes a route. A lane or route whose parameter the plugin no
  longer lists is kept and drives nothing. **Save asks every plugin for its
  state.** A change the plugin makes itself -- a gesture, values it moves, a
  `mark_dirty` -- is read off it, never sent back, and is one undo step
  ("Plugin Edit"), so undoing an earlier edit no longer reopens the plugin
  without it. A saved state the plugin refuses opens it with its defaults,
  and the song keeps the refused state unchanged.
- **Plugins are found, not yet offered** (MOO-80). At startup, on a thread of
  its own, mooloop looks for CLAP plugins in `~/.clap`, `/usr/lib/clap`,
  `/usr/lib64/clap`, `/usr/local/lib/clap` (inside a Flatpak also
  `/app/extensions/Plugins/clap`; on macOS the two
  `Library/Audio/Plug-Ins/CLAP` folders), then `CLAP_PATH`, then the
  `[plugins] extra-paths` in `settings.toml`. Each new or changed `.clap` is
  loaded only in a child process, `mooloop --scan-plugin <path>`, killed after
  `scan-timeout-s` (default 10), so a plugin that crashes or hangs while being
  scanned costs that child and nothing else. What was found, and every file
  that failed and why (`load`, `incompatible`, `crashed`, `timed-out`, ...), is
  kept in `<config>/plugins.toml`; an unchanged file is never scanned again,
  failed or not. `scan-on-startup = false` turns the startup scan off. The log
  says what the scan found. A song's plugins are found through it (above);
  the browser and the insert menu that offer them are step 08.

### Buffers And Rendering

- A loaded sample is immutable in the audio path. The only audio the
  application generates for itself is a sampler stretch commit, which
  re-renders the decoded source off-thread under a stored spec, and the
  Buffer insert's rolling ring. Neither writes a channel's own output back
  into a project asset: there is still no capture-to-sample gesture.
- The render graph is independent of the audio driver and supports finite
  offline passes at the engine's sample rate, for WAV and MP3 alike; MP3 goes
  through an in-process LAME encoder, which converts a render above 48 kHz
  to a rate MP3 has. Stem/bus export and realtime-vs-offline null testing
  are not implemented.
- Replaced sample lifetimes need a deliberate deferred-reclamation design so
  the last large sample allocation can never be freed on the realtime thread.

### Interface

- The application is usable but still has interaction and responsive-layout
  edge cases.
- **A control keeps following its parameter after you have touched it**
  (MOO-220, 2026-09-24). Before, the first drag, wheel step or reset on a
  knob, fader, toggle, segmented bank or stepper cut it loose from the value
  it showed: picking another EQ band left Freq, Gain and Q on the band just
  left, and undo, a preset load, a MIDI-mapped controller or automation moved
  the sound but not a touched control on any insert face, the source output
  trims, the sidebar's volume and pan, the mixer's faders and sends, the
  modulation shelf, or the pattern and snap fields. Every shared control now
  reports its change and draws what the document says; a knob's typed field
  (`KnobField`, `KnobStack`) shows the live value whenever it is not being
  typed into.
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
- **A contextual chord resolves against the focused surface.** Ctrl+C/X/V
  mean the notes on the roll, the selected device in the rack, and the
  channel everywhere else — the fallback they meant unconditionally before
  there was a second clipboard. The four arrow keys are the same mechanism:
  nudge or transpose on the roll, walk the tree in the browser, pick a
  channel otherwise. Which surface is focused is the roll whenever it is on
  screen with a selection, and otherwise wherever the last click landed;
  there is no Escape-to-nowhere, because selecting a channel is the way back.
  Preferences > Shortcuts says which chords are contextual.
- **The browser tree is reachable and drivable from the keyboard.** Ctrl+B
  reveals the sidebar and aims the keys at it; Up/Down move a highlighted
  row, Right opens a closed folder or steps into it, Left closes an open one
  or climbs to its parent, Enter does what clicking the row does, and
  Ctrl+Enter loads a sample or preset into the selected channel. **A move
  that lands on a sample auditions it** — the same inspection a click on it
  runs, so the info pane fills and, with the preview armed, the file plays;
  so the arrows walk a folder by ear. A row that is not a sample is only
  highlighted: a preset's click *loads* it, and a walk down the PRESETS tab
  must not install a device per keypress. An inspection decodes on a worker
  thread and several are in flight whenever the keys outrun a decode, so a
  reply about a sample the selection has already left is dropped rather than
  landing on the pane and in the speakers over the row that replaced it. It has no
  `FocusScope` of its own and deliberately does not get one — a nested scope
  swallows the pointer press that focuses it, which is the
  two-clicks-per-control bug `tests/first_click.rs` exists for. The root
  scope already hears every key; what was missing was somewhere to aim them.
- Keyboard note *selection* still does not exist: the arrow keys move an
  existing selection but cannot build one, which stays open in
  `ENHANCEMENTS.md`.
- **A channel takes a colour from a chip beside its name field in the channel
  sidebar**, which opens eleven swatches and a field that accepts any
  `#RRGGBB`, and the colour survives save and reload. It is stored as a colour
  the song owns rather than an index into the theme's palette, so a song looks
  the same under every scheme. **A pattern takes one on the same terms**, from
  the same chip beside its name field in the transport toolbar.
- **A colour is drawn wherever the thing it names is drawn, and a colour a
  thing *inherits* is drawn differently from one it owns.** A colour something
  owns is a 3px bar down the left edge of its plate: a channel's on its rack
  plate, a pattern's on its playlist gutter plate, a track's on its mixer
  strip. A colour it inherits is a wash through the whole face: a channel
  routed to a coloured track is tinted 12% with that track's colour, leaving
  its own bar free to mean its own colour. Neither indicator ever has two
  sources. A pattern's colour is also the fill of every clip that plays it. The difference is what the shape already says: a plate's background
  means "selected", so a colour beside it is a mark, while a clip's background
  only means "a clip is here" and a coloured clip still says that. A clip's
  number is drawn in black or white by the colour's luminance, so a label is
  readable on every colour that can be picked.
- One automation lane is visible at a time. Its picker reaches the selected
  channel's generator and every parameter of every effect on that channel and
  on every bus, but several lanes cannot be shown at once, the velocity lane
  is a separate fixed lane rather than one entry in that list, and a pattern
  holds at most eight lanes per channel.
- A rack device can be selected, copied, cut, pasted and duplicated. The
  selection is a `DeviceId` rather than a slot, so it follows its device
  through a reorder and clears when the device is removed; clicking a device's
  header selects it and clicking it again clears. Copy takes a container's
  whole run, the same unit it is deleted and saved as, and strips identity, so
  a paste is a new device that sounds the same rather than the same device
  twice. A paste lands after the run it was dropped on, and a run's end
  boundary is outside a container, so pasting onto a box's last child lands
  beside the box rather than in it. **Duplicate does not follow that rule**:
  a copy lands at the original's own depth, so duplicating a box's last child
  keeps it in the box. Paste is aimed at a *position* and duplicate is aimed
  at a *row*, and only the second one can say which side of the boundary it
  meant. Duplicate is on every rack row's left rail; all four are on
  Ctrl+Shift+C/X/V/D, and all but copy are undoable.
  **A pasted channel arrives with its MIDI and AUDIO inputs off**: the picks
  name something in the document the channel was copied from, and the
  clipboard outlives New Song and Open Song.
  **The clipboard does not carry modulation routes or automation lanes**: a
  route's source is a module in the channel's own rack, so it cannot follow a
  device to another channel. That is the question `docs/plans/archive/containers/`
  reserved rather than answered, and this inherits its answer.
- A canonical action registry drives the menu bar and rebindable shortcuts.
  Note multi-selection supports Select All and bulk deletion. **Undo covers
  every edit that changes the document**, on one project-snapshot undo/redo
  stack: channel structure, pattern verbs, note and step-grid edits, pattern
  length, playlist placements, both renames, modulation, automation, presets
  and kits, sample loads, the sampler's slice verbs, the mixer's own verbs,
  every device and generator parameter, and what arrives from MIDI -- a
  learned binding, a mapped control's moves, and notes recorded from a
  keyboard. Only opening or starting a song clears the history; a kit load
  does not, because it edits the song that is open.

  **One gesture is one undo step.** A knob emits a value on every pointer
  frame, so the control says where a gesture starts and stops -- `Gesture` in
  `controls.slint`, which every shared widget calls and no device face knows
  about. A knob drag, a fader drag, a painted run of steps, a dragged
  envelope handle and a typed rename are each a single Ctrl+Z, whatever their
  length; a wheel notch, an arrow-key nudge, a double-click reset and a menu
  pick are each one on their own -- except that wheel notches, arrow presses
  and resets on one control less than half a second apart join one step, so
  a trackpad sweep of a cutoff is one Ctrl+Z rather than forty. A press that
  moved nothing records nothing, and naming a control for MIDI learn is not
  an edit.

  **What has no press and release is bracketed by what it is.** A mapped
  hardware control's moves are one step, closed when the desk has been
  still for half a second; a take of notes recorded from a keyboard is one
  step, closed when the transport stops or recording is disarmed; a learned
  binding and a sample load are one step each. Undo while a knob is still
  settling or a take is still running undoes that stream, not the edit
  before it.

  `scripts/dupe-audit unrecorded-edit` is the guard: it reports every
  callback whose handler changes the document without recording it, and
  every place the pump does, and its expected answer is zero. Project-level
  navigation remains limited.

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
