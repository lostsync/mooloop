# Interface

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## The window and its panes

- One application window with a transport toolbar, a work surface, and a lower
  dock. The transport row carries play/stop, pattern-vs-song mode, a
  bar:beat:tick position readout, beat lamps, drag-or-type tempo, global
  sixteenth-note swing, and the master meter, and never changes.
- **The work area is five views in three slots.** `main` and `split` divide
  the top; `bottom` is the dock. A view lives in exactly one slot, and a
  slot's tab strip lists what it holds, so no strip can misreport what is on
  screen. Each slot's rectangle is computed rather than nested in layouts,
  which is what lets a view be drawn anywhere off one instance.
- **A channel sidebar flanks the work area on the left.** It holds the
  selected **channel or track**'s name and colour -- following the same
  selection the device rack does, so the two cannot describe different
  things -- plus three MIDI rows for a channel: IN and CH are live, and OUT is
  inert because MIDI output does not exist. A channel also carries an
  **OUTPUT** picker naming the mixer track it feeds. It is the same edit the
  rack row's chip makes, reading the same model row, so the two cannot
  disagree -- but it says `Bus 3` where a 30px chip in a run of steps can only
  say an arrow and a number, and it is to hand when the step grid is not on
  screen. A track's destination is not there: it lives on the track's own
  mixer strip, beside the analog-sum switch it is a property of. A track draws
  no MIDI rows at all rather than disabled ones: disabled means "not
  configurable yet", which is true of OUT and would be a lie about a track,
  which has no MIDI input to configure. It is hidden until the status bar's
  leftmost chip, **View > Channel Sidebar** or **Ctrl+[**
  (`view.channel-sidebar-toggle`) opens it. At the bottom it has a **MIX**
  row -- mute, solo, volume and pan -- for the channel or the track it shows.
  The rack row keeps its own four, and both drive the same verbs, so moving
  one moves the other. For a track it also holds that track's **sends**: one
  row each with destination, pre/post tap, enable, remove and a level
  fader the width of the row, plus a
  `Send to…` picker that routes a copy to another track. The mixer strip
  keeps only the levels, as one bar per send reading `3 REVERB  -6.0`: drag it
  (relatively; Ctrl for fine), double-click for unity, Shift+click to switch
  the send off or on, Alt+click to remove it. The tap point and the remove
  button are the sidebar's alone. It resizes by its right edge between 180 and
  400px, remembers its width, and edits whatever channel is selected rather
  than holding a selection of its own. The
  `reference/img/mooloop-1.0-mockup.png` panel also draws PLUGINS and MIXER
  tabs; those are second views of the rack and the mixer and are deliberately
  not built.
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
  268px and does not stretch.
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
  shows that view wherever it lives.
- **A view has exactly one toolbar row, and its slot's tab strip leads it.**
  With `STEPS` up it carries pattern selection, the cursor tools and pattern
  length; with `MIXER` up, nothing, because the mixer's controls are on its
  strips; with `DEVICES` up, the device chain's source picker and, at the far
  end, the field that renames the channel and its preset browser. The preset
  browser is on `DEVICES` alone; on the piano roll a whole-channel preset
  browser was noise.
- **Each editor owns its own grid snap**, in its own row: the piano roll a
  toggle and a menu, the playlist a menu.

## Status bar, furniture, preferences and menus

- **The status bar holds a failure until it is read.** A warning
  or an error has its own segment at the bar's left, in amber or red, and
  stays there until it is clicked away or a notice of at least its weight
  replaces it -- the next "Device selected" goes to the ordinary line beside
  it instead of over it. A sample that will not decode, a load onto a channel
  that is not a sampler, a new-channel load with no room, a failed preview,
  a take that could not be written or has nowhere to land, and a device
  whose NaN output the master's guard is silencing all arrive there. So do a
  device that panicked and was silenced, and "No audio": when the audio stops
  or never started, that notice stays after the Reconnect question is
  dismissed, and comes down by itself when the audio is running again.
  The bar's right end reads the audio callback once a second: its load as a
  share of the block budget, a dropout count in amber that a click resets,
  and a "not realtime" badge when the callback thread is ordinary rather than
  realtime. The badge reads the thread's policy at each refresh, so
  it clears when the driver promotes the thread after its first block and
  appears if the thread is demoted mid-session. It shows "DSP –" while no audio is heard, rather than the null
  driver's timing. Hovering either segment explains it in the hint line.
  **A slow callback says where it was**: a callback that uses more than 60%
  of its budget records the song position and the three channels or tracks
  that took most of it. The readout shows the last one in amber beside it,
  "bar 12.3 · Bass 41%, Drums 22%", each figure a share of the block's
  budget, in a fixed-width box that elides rather than moving the bar, with
  the explanation in the hint line on hover; it holds until the readout is
  clicked, and the log gets one line a second while it keeps happening. An
  export does not time anything.
  `status_bar::notify` in `ui/src/status_bar.rs` is the one door.

- The window holds three pieces of furniture around the work area, none of
  which is a dockable-pane system: an always-visible docked status bar
  carrying hover hints and the panel toggles, a draggable splitter that
  resizes and collapses the lower editor dock, and a right-hand browser
  sidebar on a resize grip. The sidebar browser has three tabs over one row
  model, SAMPLES, PRESETS and PLUGINS (the last is in [plugins.md](plugins.md)). **Samples**: persisted locations added through
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
  move, even when embedding rewrites where the bytes are. A song opened from
  disk has no browse folder to remember and steps through its bundle, which
  is all the document knows. **Presets**: every well-known preset directory
  scanned on entry to the tab and grouped — Channels, then one group per
  device kind, then one per effect kind, empty groups omitted — each group
  expanding to its presets with a count beside it, and a preset's category
  and tags shown when they say something its group does not. **A click
  selects a preset; a double-click, Enter or a drop onto the rack loads it**,
  and a right-click offers Load and, for an effect preset, Add as New
  Device. An effect preset added as a new device fades in where it lands like
  any inserted device, and nothing else on the song stops or is rebuilt for
  it. The click also **auditions an instrument or channel preset**: the
  preset is rendered offline, off the UI thread, into a short phrase (a bar
  of hits for the drum devices, an arpeggio into a chord for the pitched
  ones, root and fifth for a sampler) and played through the preview voice
  when autoplay is on, like a clicked sample. The song is not touched. An
  effect preset's click only selects, and that is Adam's ruling (no effect
  audition). A **filter** field above both tabs narrows the
  tree by words: presets by name, category, tags and group, opening every
  group that has a match; samples by file name, searched through every folder
  under every location rather than only the open ones (bounded, on the UI
  thread), listed with the folder each sits in. A channel preset replaces the selected channel and a
  generator preset replaces its source device, which is why a generator
  preset is offered only on a channel already holding that kind and is drawn
  greyed otherwise. **An effect preset appends a device to the end of the
  chain the rack is showing -- a channel's or a mixer track's -- rather than
  replacing one**, and the rack stays there with the new device selected. So
  it is always loadable -- unless the selected device is that preset's kind,
  when it loads into that device instead, which is Adam's rule for the
  double-click; the rack row's own rail is still where a preset replaces what
  is already in any row. **Every preset load is one undoable edit**, effect,
  channel and generator alike, and so is a kit load; none of them stops the
  song, because each edits the song that is playing rather than opening
  another.
- A two-pane Preferences dialog with General, Audio, MIDI, Appearance,
  Shortcuts and Plugins pages; General persists developer mode and reveals the presently
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
  bring square corners and a bevel with them. A further row,
  **Wallpaper**, appears whenever pywal or wallust has left a palette in
  `~/.cache/wal/` or `~/.cache/wallust/`, and is that palette. A variant the
  scheme does not publish is derived from the one it does and says so in the
  list. A **Dark / Light / Auto** control picks the side, where Auto follows
  the desktop's own `org.freedesktop.appearance color-scheme` (the system
  appearance on macOS), **live**: switching the desktop between light and dark
  switches mooloop within a frame, with Preferences open or closed.
  **The three colour pickers' quick swatches come from the scheme the page
  names**, even after a swatch has made the colours Custom: Base
  offers its three background-like neutrals (slots 00-02), Accent its eight
  hues plus its own accent when that is none of them, and Alert its eight
  hues. So a built-in's own three seeds are each a selectable swatch.
  **The channel, track and pattern swatch palette follows the theme**: eleven
  colours taken from the ramp's eight hues plus a midpoint in each of its
  three widest gaps. A song stores the colour it was given rather than a
  palette index, so changing themes never repaints anybody's channels.
  Beside the colours: roundness, contrast, **text size** (65-200%; 100% is
  what read as 115% before 0.1.6, and a stored size is read against the new
  base rather than migrated), **border and emphasis widths**, **relief**
  (flat, bevel or inset, with a depth, and taken away again by a theme that
  doesn't state one). A bevel draws on the buttons, toggles, segmented
  selectors and tabs, the knobs' caps (lit from the other side while
  dragged), each device's plate and header and a folded device, every pane
  including the dock, the toolbar, the status bar and both sidebars;
  dividers, rails and the inside of faces stay flat. And
  **two font families** -- one for the
  interface and one for readouts. A font that is not installed falls back to
  the platform default, because mooloop registers no fonts at runtime (Slint
  1.18.1 can do it only behind an unstable feature, which mooloop leaves off)
  and a theme can only name a family. Themes save to
  `<config>/mooloop/themes/<name>.toml`, one file per theme, and a malformed
  one is skipped with a message rather than stopping startup. **Density** has
  no control on the page, but a theme that states one (Impulse)
  still applies it and the setting still persists. All of it
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
  chord asks what was clicked last. The recorder accepts an unmodified key,
  so the shipped bare-L and bare-digit defaults can be put back after a
  Reset.
- A traditional menu bar above the toolbar (`menubar.slint`): File, Edit,
  Pattern, Channel, Track, View, and Help. Menus are declared where their window
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
  adds to that directory and never rebuilds it**: a file
  already there keeps its place and its name and is not copied again, and a
  file the song stops using stays until File > Clean Up Takes moves it to
  the trash, so an undo back to it still finds it. Older
  directory-style song bundles remain loadable and migrate when resaved.
  Missing or corrupt samples warn and load as silent slots. **Renaming a song
  and its assets folder together, in a file manager, works**: the document
  still names the old folder, and the loader reads this song's own instead and
  says so, which the next save writes back.

## Widgets

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
  build carrying that feature. It is off by default because exporting it from
  the window compiles it into every build (everything one `.slint` entry point
  reaches becomes a single generated Rust module); the Developer page hides
  the row when it is absent. Its palette comes from one
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

## Interaction, keyboard, colour, selection and undo

- The application is usable but still has interaction and responsive-layout
  edge cases.
- **A control keeps following its parameter after you have touched it.**
  Every shared control -- knob, fader, toggle, segmented bank or stepper --
  reports its change and draws what the document says, so picking another EQ
  band moves Freq, Gain and Q to that band, and undo, a preset load, a
  MIDI-mapped controller or automation moves a touched control along with the
  sound: on any insert face, the source output trims, the sidebar's volume
  and pan, the mixer's faders and sends, the modulation shelf, and the
  pattern and snap fields. A knob's typed field (`KnobField`, `KnobStack`)
  shows the live value whenever it is not being typed into.
- **A shortcut fires from wherever focus happens to be.** The root
  `FocusScope` surrounds the UI, with `focus-on-click: false` so it does not
  swallow the presses that reach the controls inside it. `ToolButton` does
  not accept Space, so clicking a toggle, a segmented control, a pane tab or
  a mute button leaves no caret that re-fires it instead of starting the
  transport. Space is the transport; Enter activates a focused button.
- **A contextual chord resolves against the focused surface.** Ctrl+C/X/V
  mean the notes on the roll, the selected device in the rack, and the
  channel everywhere else. The four arrow keys are the same mechanism:
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
  landing on the pane and in the speakers over the row that replaced it. It
  has no `FocusScope` of its own and deliberately does not get one — a nested
  scope swallows the pointer press that focuses it, which is the
  two-clicks-per-control bug `tests/first_click.rs` exists for.
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
  **A copied, pasted or duplicated plugin device is a second instance of
  its plugin**, in a slot of its own, opened with the copied
  plugin's state: a duplicate takes the state the plugin holds now, a copy
  the state its last finished edit left in the song. It lands the same way
  in another song. A container copied with a plugin inside
  carries it the same way. **A container preset carries the plugins inside
  it**, with the state each held when it was saved, and loads them
  into slots of their own; 0.1.5 refuses such a preset. One saved before
  that, holding a plugin, is refused rather than landing on whatever the
  target song had at the plugin's old slot number.
  **A pasted or cloned channel runs its own instances of its plugins**:
  its instrument and every plugin on its chain, inside
  containers too, each in a slot of its own, with the state the plugin held
  when the channel was copied.
  **Pasting or cloning a channel stops nothing**: every other channel keeps
  its sounding notes, tails and modulators, and the pasted one arrives with
  its notes, lanes, chain and modulation as an opened song would have it.
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
