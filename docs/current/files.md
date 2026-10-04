# Files: export, saving and loading

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## Export

- Offline export of one pattern pass, the song, or a range of it (below),
  followed by a release tail that runs
  until every device has fallen silent (`RenderState::is_at_rest`, so a
  reverb's decay and a delay's last echo are waited for), capped at a
  0-30 second limit the dialog sets (10 s by default). Outputs are WAV at
  16-bit PCM, 24-bit PCM or 32-bit float, or 192/256/320 kbps MP3, each
  stereo or mono.

  **The export dialog** is five sections, Source, Range, Format, Tail and
  Output, and stays up through the render with a progress bar and a Cancel,
  which stops the render and leaves any file already at the target
  untouched; then it shows the file's length and rate and every count below.
  A finished file replaces the target with one rename. The Format row is WAV
  or MP3 with Stereo or Mono, then a WAV's depth and a Dither box, or an
  MP3's bitrate. Dither is TPDF, on by default at 16-bit and off at 24,
  and never applied to float. Its seed comes from the file's place in
  the job: renders are bit-identical, and two files of one job carry
  unrelated dither. A mono file is (L + R) / 2: a centred sound
  keeps its per-side level, and a hard-panned one is 6 dB down
  (`GAIN_STRUCTURE.md`, "Mono files"). An MP3 in mono uses LAME's mono
  mode.

  **Output** has no save-file chooser: it is a folder field with Browse... (a
  folder chooser) and a file-name field on the card itself. An empty folder
  is the song's own folder, or the music folder (`XDG_MUSIC_DIR`, else
  `~/Music`) for a song never saved; an empty name is the song's name. A
  folder that is not there is said on the card before anything renders. An
  export **never replaces a file by default**: "Don't overwrite: number it",
  under the name, is on, and a name already in the folder is written as
  `song-001.wav`, then `-002`, three digits before the extension. The files of
  one export share one number, the lowest free for all of them, and a typed
  number is not read (`song-001` numbers to `song-001-001`). The line beside
  the box says the file that will be written. A file that appears under the
  chosen name while the export renders is left alone, and the export takes
  the next free number instead: the last step is a rename that refuses to
  replace (a hard link, or a claimed new file and a copy where the drive has
  no hard links). With the box off, an export that would replace files asks
  once, saying how many.

  **Source** is the master mix, **Tracks** or **Channels**. Tracks shows a
  checklist of every track but the master, with a bus's feeders indented
  under it, plus All / None, "+ master" and "Skip silent". The tracks feeding
  the master are checked by default: their stems summed are the master's
  input. Each checked track's own output (after its rack, fader and balance,
  before what it feeds, as a console's direct out) goes to
  `<name>-<track name>` in one pass, and every file of the export shares the
  one number. Every stem is as long as the master, because the tail runs
  until the whole song is at rest. A muted or solo-silenced track's stem is
  silence, and with Skip silent it is not written (that is logged; the result
  card does not list it). No limiter runs on a stem: an over is written as it
  is in float, clamped in PCM, and counted per file, so float is the format
  for stems. Channels lists every channel, checked, and each checked one's
  own output (its source, rack, fader and pan, bypassing the mixer) goes to
  `<name>-<channel name>` in the same single pass. A channel on a muted track
  still renders; a muted channel renders silence.

  **Range** is a choice of four: the whole song, the loop selection (its
  points whether or not looping is on; unavailable while the song has none),
  a custom range typed as bar.beat (or bar.beat.sixteenth), which starts as
  the loop selection, and the current pattern, one pass. The card shows the
  stretch each covers under the choice, follows the transport (song or
  pattern) until a range is picked, and refuses a custom range that ends at
  or before its start or past the song, with Export disabled and the reason
  on the card. A range is **the same frames as the whole song**: the render
  plays from the song's top with nothing written until the range's first
  frame, so a reverb or delay from before the range is in the file,
  automation and tempo-synced LFOs read what playing through reads, and a
  note that began before the range sounds from its first sample. There is no
  setting: the pre-roll is always from the top, and costs an offline render
  of the bars before the range.

  The card **remembers how the last export was delivered**, across launches
  and in every song: the format, a WAV's depth and dither, an MP3's bitrate
  (both, whichever was exported), stereo or mono, the tail, and "Don't
  overwrite: number it". They are saved in `settings.toml`'s `[export]` table
  when an export starts, and the card opens on them each time, so an edit on
  a card that was cancelled is not kept. An entry that will not read, or a
  format this build does not know, is its default and a line in the log, and
  never costs the rest of the settings. The folder, name, source and range
  belong to the song and are not carried between songs (saving them in the
  song: MOO-194).

  **The render.** An export is a job of passes, each pass one render handed
  to every file it writes, under one progress bar and one Cancel; cancelling
  keeps the files of passes that had finished. It renders in 512-frame
  blocks, a size live playback runs at, rather than the graph's 8192-frame
  maximum, where one automated parameter could fill a device's event list.
  Parameter events that still find no room are counted, and an export that
  lost any logs how many. It renders with subnormals flushed to zero, as the
  audio callback does, and its render thread's floating-point mode is
  restored afterwards, so an export and playback through the executor agree
  to the bit. The render passes through the same output guard as playback, so
  a file never holds NaN or a sample over 0 dBFS; `RenderSummary` counts the
  mix's overs (which the safety limiter held at the ceiling), the non-finite
  samples written as silence, and any sample the PCM encoder still had to
  clamp, and a non-zero count is logged. Every format renders at the
  session's rate: an MP3 of a session faster than 48 kHz is rendered at the
  session's rate and converted by LAME as it encodes (88.2/176.4 kHz to 44.1,
  the rest to 48).

## State and persistence

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
  Cancel**: Save runs the ordinary save, and goes on to quit, open or start
  the new song only once the save has succeeded -- a failed or cancelled save
  leaves you where you were. Quit and the close button ask it in the same
  words. Replacing a preset of the same name and loading a kit that drops
  channels holding notes are asked in the same dialog, and none of them blocks
  the UI thread. **A file chooser is asked of the desktop's file chooser
  portal first** (`org.freedesktop.portal.FileChooser`, which KDE, GNOME and
  most tiling setups provide), then of `zenity`, then of `kdialog`; on macOS
  it is the system's own panels through `osascript`. **When none of them can
  show one, that is not a cancel**: Save, Save As, Open, Export and the kit,
  channel and bundle pickers raise the error dialog, listing what was tried
  and what to install, and Load Sample and Add Folder say so in the status
  bar.
- Missing samples are recoverable by loading a replacement audio file, but
  there is no dedicated path-search/relink dialog yet.
- **Unsaved changes survive a crash.** Once a minute, while the
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
- **A file operation that crashes does not lock the File menu.** A panic in
  an open, save, load or export worker (a sample decoder, say) is reported in
  the error dialog as an internal error, and the menu works again.
- **Every run leaves a trace.** The diagnostic log is written on every run to
  `~/.local/state/mooloop/mooloop.log` (`$XDG_STATE_HOME`; `~/Library/Logs/mooloop/`
  on a Mac), rolled aside past 4 MB, and Preferences > Developer shows where;
  it is not a preference. A panic leaves a crash report with a
  backtrace in `crashes/` beside it. SIGTERM, SIGINT and SIGHUP -- a logout,
  `kill`, Ctrl+C -- quit through the quit path without its dialog: the log
  says which signal, a take still recording is finished, and unsaved song
  changes are not saved but are kept in the autosave for the next launch to
  offer. It does not write a last autosave on the way out either, by Adam's
  ruling: *"if you received a kill there's no telling what the situation is.
  write might fail and corrupt your autosave"*. `docs/OPERATIONS.md`,
  "Diagnostic Log", has the details.
- **A song old enough to reference the built-in kick opens with it audible.**
  Projects saved before the sampler stopped auto-loading a kick carry a
  `SampleReference::Builtin`, which the install substitutes the cached default
  for.
- **Two sample loads into one channel resolve in the order they were asked
  for, not the order they finish.** Each dispatch carries a request token and
  a superseded completion is discarded, so a long file chosen first cannot
  land last and win.
