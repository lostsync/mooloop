# Plugins

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

- **A CLAP effect goes in a chain from the window.** The join's menu ends in
  **Plugin…**, which opens the browser's third tab, **PLUGINS**, aimed at
  that join. The tab lists what the scanner found (`<config>/plugins.toml`,
  re-read whenever the tab is opened, which is how a scan that finished after
  startup shows up), one row per plugin with its vendor, filtered like the
  presets by name, vendor or what the row says. A plugin that cannot go in a
  chain is greyed with the reason: an effect whose ports are not one input
  and one output of one or two channels, or one the factory could not
  create; so is each file that failed to scan, with why. **Preferences >
  Plugins' "Hide plugins mooloop can't use yet"** (off by default, saved)
  leaves out the plugins refused as *unsupported* -- they loaded, but a main
  port is not one or two channels, an instrument takes no notes, or it is the
  other role's -- and never one that *failed*: a plugin that could not be
  created and a file that failed to scan stay listed, greyed with the reason,
  so a plugin that breaks after an update never quietly disappears.
  A double-click, Enter or a drop puts
  the plugin in the chain: a drop before the join it lands on, otherwise
  before the join the menu was opened from, otherwise after the selected
  device (and the run a selected container holds), otherwise at the end.
  Adding it is one undo step ("Plugin added"). An instrument goes on a new
  channel instead (below).
- **A plugin with no GUI has a face**: its parameters, in the
  plugin's order and under its own names, as the knobs every native device
  uses, with the plugin's own text for each value ("7.2 dB"). **The face
  shows the pinned parameters**: the first eight the plugin does
  not hide until something is pinned, one unit wide while they fit three
  across and two rows, two units beyond that, with `<` `>` between pages.
  **The rest are in the sidebar**: with a plugin device selected, the
  channel sidebar's PARAMETERS list shows every parameter the plugin does
  not hide, under its group, with a field that filters by name or group and
  a pin on each row that puts it on the face or takes it off. A pin is saved
  with the song and is one undo step ("Pin Parameter"); the first one starts
  from the eight the face already showed. A pinned parameter the plugin
  stops listing is kept and not drawn. **A stepped parameter the plugin
  names at every position** (2 to 8 positions at whole values, each with a
  name of its own, and no other name between two neighbours) is a row of
  segments labelled with the plugin's words; any other stepped parameter
  is a knob that lands only on its positions, which is how LSP's filter
  type, which names more choices than it reports positions, stays
  reachable. Parameters the plugin groups (CLAP's `module` path) carry the
  group's name as a small caption over their run on the face. The knobs follow the song: undo, a lane, a preset and the
  plugin's own edits all move them. A turn or a drag is one undo step
  ("Plugin Edit"), recorded once the plugin has gone quiet after the release,
  however long the drag paused. A **missing** plugin keeps its face, drawn
  from the parameter list the song remembers, greyed, with a badge saying it
  is missing and plays dry; a plugin that failed says why. **A plugin knob
  is a destination like a native one**: with a modulator armed it
  authors a route on the plugin's own parameter and draws the ring, the
  offset and the route dots, and its context menu names it for a lane or a
  MIDI mapping. **MIDI learn on a plugin knob works**: the
  mapping list names the parameter ("Drums · Test Gain 1 · Gain") and the
  control moves it, picking it up from the plugin's current value. A sweep of
  such a control is one undo step, the plugin's own "Plugin Edit". The lane
  picker lists a plugin instrument's parameters at
  the head, under the plugin's name ("Test Sine"), before the inserts, and
  each plugin effect's after the native inserts and before the strip,
  under its name and chain place ("Test Gain 1"). The shelf
  names a route on one the same way. **A parameter the plugin
  stops listing is kept** (Adam's ruling): its lane stays in the picker,
  titled thin and italic by its id ("Parameter 4000000000"), the lane and
  its shelf row draw greyed, and it plays nothing; when the plugin lists the
  id again they read normally, with nothing to repair. **A plugin
  channel's source has the same face**, in the source's
  place at the head of the chain, under a SOURCE header named after the
  plugin, one or two units wide as the face would be on a chain: its pinned
  parameters as knobs, the open-window button when the plugin has a GUI,
  the badge when it is missing. Selecting the source header puts the
  instrument's parameters in the sidebar's PARAMETERS list, to find and pin
  ("Pin Parameter", saved with the song). Its knobs set the instrument's
  parameters, arm routes, MIDI-learn and name themselves for a lane or a
  mapping, all as `PluginParam` on the channel's source device.
- **A plugin device saves and loads presets** from its rail, like
  any device. A preset keeps which plugin it is and the state the plugin
  holds at that moment, including a knob just turned in its own window. It
  is kept under `presets/effects/plugin/<vendor>/<id>/`, and a plugin
  device's menu offers only that plugin's presets. Loading one, in this song
  or another, opens the plugin with the preset's state in a new slot while
  the device keeps its lanes and routes. It is one undo step ("Effect preset
  loaded"). A 0.1.5 build refuses these presets. Not yet: plugin presets in
  the browser's PRESETS tab, and the plugin's own CLAP factory presets.
- **A CLAP effect plays in a chain.** Besides
  the window (above), a plugin reaches a song from a song that already
  names it: a `plugin` effect device whose slot is in the song's `plugins`
  table (`PROJECT_FORMAT.md`, "Hosted plugins"), opened from disk or the
  command line. `Session::insert_plugin_effect` is the one path the window,
  the tests and `crates/mooloop-session/examples/clap_effect_case.rs` share. mooloop finds the
  plugin by its id in the scanner's cache (`<config>/plugins.toml`, below),
  rereading the cache when a scan rewrites it. It loads the plugin with its
  saved state and activates it at the engine's rate. On the next pump tick it
  swaps the plugin's processor into the device, which played as a
  pass-through until then. A plugin with one input and one output, each
  mono or stereo, is hosted. A **mono effect
  runs**, like a TRS cable into a TS jack, with no setting: a mono input
  hears the chain as `(L + R) / 2`, so a centred signal passes at unity and
  a hard-panned one comes through 6 dB down, and a mono output is copied to
  both sides. The dry path and wet/dry stay stereo. A plugin with **extra
  ports** is hosted as well, as long as its
  main input and main output are mono or stereo. Those are the ports it
  flags as main, wherever they sit. A sidechain or any other extra input
  hears silence, and an extra output (Surge XT's scenes, say) is thrown
  away. Nothing can feed a sidechain yet: that is 0.2.0. A plugin whose
  main ports are wider is refused as unsupported. The plugin is heard live, and **it is in an export**: the export
  renders with second instances opened from the live ones' state. A plugin
  that reports an error or a non-finite sample, or panics, is passed through
  from that block on. A song whose plugin is **missing** opens, plays that
  device as a pass-through, and saves it back unchanged, lanes and routes on
  its parameters included. It is tried again when a scan finds new plugins.
  A structural edit (paste, move, delete, undo) keeps a hosted plugin
  running, state and all. A new sample rate, or a restart the plugin asks
  for, rebuilds its processor. **The rebuild fades rather than clicks.**
  The plugin fades out to the dry signal over about 35 ms, the
  device plays dry, still as late as the plugin was, while the new
  processor is built, and then the plugin fades back in. A missing plugin
  that turns up fades in the same way, unless it reports latency: then the
  channel moves later by that much when it arrives, as it does for any
  latency change. On quit, mooloop waits up to two seconds for
  every plugin's processor to come back from the audio thread before it
  destroys the plugin, and any GUI it has open is destroyed first.
- **A plugin with a GUI of its own opens it from its face.** The face's foot
  has an open-window button, drawn only
  for a running plugin that has a GUI; pressed, the plugin's GUI opens in a
  bare X11 window of mooloop's (XWayland under Wayland), titled with the
  plugin's and the track's names, sized as the plugin asks and scaled as the
  main window is; pressed again, it brings that window to the front. A
  plugin that only floats opens its own window. The window's close button
  closes the GUI and leaves the plugin playing. Removing the device, New,
  Open and quit close the GUI, then its window. **On a native Wayland
  session** the plugin windows hide while neither mooloop's window nor any
  plugin window has focus (after 0.3 s, so a click from one to the other is
  not taken for leaving), and come back when mooloop is focused; on X11, or
  with "Run under XWayland" on (below), each is kept above the main window
  and nothing hides. A window that cannot open (no X server, a plugin that
  refuses) is said in the face's badge, and the face stays. Every pump tick
  services each plugin's timers and fds, which is what a Linux plugin GUI
  runs on. A plugin channel's instrument opens its GUI the same way, from
  the open-window button on its source face.
  **On macOS** the window is a panel of mooloop's own that the plugin's
  Cocoa GUI is placed in, titled the same way, sized in points as the
  plugin asks; it floats above mooloop's windows and hides while mooloop is
  not the active application, so mooloop never hides it itself; its close
  button closes the GUI as on Linux. **A knob turned in the GUI
  moves the face's, and the face's moves the GUI's, while the plugin is not
  processing** -- asleep in silence, bypassed, on a muted channel, an
  instrument muted or idle: the engine flushes such
  a plugin (CLAP's `params.flush`, on the audio thread) the block it has a
  face edit waiting or asks for one (`request_flush`).
- **A hosted plugin's parameters take lanes and routes.** The face's knobs
  author them (above). A lane sets
  the parameter at every 32-frame control tick, in an export and live alike;
  a route is an offset over the plugin's own value (CLAP's parameter
  modulation), gone when the route goes. Only a parameter the plugin marks
  automatable takes a new lane, and only a continuous one it marks
  modulatable takes a route. A lane or route whose parameter the plugin no
  longer lists is kept and drives nothing. **Save asks every plugin for its
  state.** A change the plugin makes itself -- a gesture, values it moves, a
  `mark_dirty` -- is read off it, never sent back, and is one undo step
  ("Plugin Edit"), so undoing an earlier edit does not reopen the plugin
  without it. A saved state the plugin refuses opens it with its defaults,
  and the song keeps the refused state unchanged.
- **A channel's instrument can be a hosted plugin.** From the window: the
  channel rack's `+` menu ends in **Add Plugin…**,
  which opens the browser's PLUGINS tab, and a double-click or Enter on an
  instrument there adds a new channel, named after the plugin and selected,
  whose source is that plugin, as one undo step ("Plugin channel added"). A
  plugin channel also comes from a song file that names one
  (`source.type = "plugin"`, `PROJECT_FORMAT.md`), or from the session call
  `Session::set_plugin_source(channel, plugin)`. The channel is silent until
  the plugin opens, and stays silent, keeping its slot and state, while the
  plugin is missing. It is
  in an export, it survives an edit that keeps the channel, and a restart or a
  new sample rate pulls the plugin out and puts the next processor back.
  **That swap fades rather than clicks**: the instrument fades to
  silence over about 35 ms before its processor leaves, and the next one
  fades in. The notes it was holding end with the fade and are not struck
  again on the new processor; the next note-on plays as usual. A plugin
  channel's editor shows the plugin's face (see the
  plugin face above), with knobs to turn, route and MIDI-learn from; its
  number is 8, after the eight native kinds, and "Add Plugin…" is its own
  row, not one of the eight. Not yet: the plugin's own latency is not
  compensated. Replacing a plugin instrument forgets the
  lanes and routes on its parameters, as deleting an effect does; undo brings
  them back.
- **A CLAP instrument plays its channel's notes**, from the pattern, a
  keyboard or an audition, each at its own frame,
  live and in an export alike. Where a plugin may go is its own word: one
  that declares itself an instrument (or declares neither and takes notes)
  can be a channel's source if it has a note input and one output of one or
  two channels (a mono one is heard on both sides); an audio input, if it
  has one, is fed silence. One that declares itself an effect goes in a
  chain, with a stereo input and output; one that declares both goes in
  either. Put anywhere else, it is refused with the reason and the song
  keeps it. A choke releases every note it holds, a stop resets it, and a
  note-off for a note it is no longer holding is dropped. Notes a plugin
  plays of its own are counted, not routed. Plugins that take only MIDI get
  MIDI note messages. The plugin browser marks what will open where from the
  same rule.
- **Plugins are found at startup.** At startup, on a thread of
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
  failed or not. `scan-on-startup = false` turns the startup scan off.
  `run-under-xwayland = true` (off by default; Preferences > Plugins' "Run
  under XWayland (full plugin window behaviour)") runs the whole
  application on X11 through XWayland under a Wayland session, from the next
  start, so plugin windows are kept above the main window; with no X server
  it stays on Wayland and logs why. Under it the window is drawn
  at the scale `WINIT_X11_SCALE_FACTOR` names, else the X server's
  `Xft.dpi`, else a whole `GDK_SCALE`, else 1 -- never one worked out from
  the screen's reported millimetres, which draws it too large on a high-dpi
  laptop panel. The log says which. On macOS the setting
  does nothing: mooloop always runs on AppKit there, and
  Preferences > Plugins does not show it.
  The log
  says what the scan found. A song's plugins are found through it, and the
  browser's PLUGINS tab lists it (above). **Preferences > Plugins**
  shows the standard folders, the added ones (Add Folder…, and a × to
  remove one), the timeout, the startup switch, the hide switch with how
  many plugins it hides (or shows greyed), and under FAILED TO LOAD every
  plugin that could not be created and every file that could not be read,
  with the reason; each edit is saved as it is made. **Rescan
  All** forgets the failures and scans every file again on the scan's own
  thread, with its progress on the page and in the status bar; when it ends
  the PLUGINS tab and the failure list are read again. Only one scan runs at
  a time: a Rescan All while the startup scan is still going says so and
  starts nothing. The same rescan is the bindable action `plugins.rescan`,
  and `browser.plugins` opens the PLUGINS tab.
