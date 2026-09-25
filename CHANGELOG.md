# Changelog

What changed for someone making music with mooloop, grouped by area. Internal
refactors, tests, CI and tooling are left out unless they change what ships.
`scripts/release-notes` prints the full commit list for any release.

## 0.1.5 — unreleased (changes since 0.1.4, 2026-09-18 to 2026-09-25)

The biggest release so far. It adds plugin hosting, audio recording, a
rebuilt export and the sampler's loop tools, and it takes most of the CPU the
interface used to spend. A song that dropped out constantly on 0.1.4 now
plays.

**Heads up:** ML-P8 Unison no longer gets louder as voices are added, so a
song that uses it plays quieter than it did (one test song's ML-P8 channel
dropped about 9.6 dB). Turn that channel up to match.

### Highlights

- **Much lighter on the CPU.** The window used to take a whole core while
  stopped on a busy song. It now takes about 2%, and draws 24–30 fps while
  playing instead of 7–10. Audio rendering is slightly faster than it was in
  mid-September, and several devices cost a fraction of what they did (see
  *Performance*).
- **CLAP plugins.** mooloop can host CLAP instruments and effects: they play,
  save, reopen and export, their parameters take automation lanes and
  modulation routes, and a plugin with no GUI of its own gets a face. The
  plugin scanner runs every library in a separate process and caches the
  result, so a bad plugin can't take the app down.
- **Audio recording.** A channel can record its audio input into a take, and
  a finished take becomes that channel's sample. The sampler has a RECORD
  page, inputs can be monitored and metered, and Core Audio takes input on
  macOS.
- **Export, rebuilt.** Render the whole song, the loop selection, a bar.beat
  range or the current pattern. Export stems (one file per mixer track) or raw
  channels (one file per channel, bypassing the mixer), all in one pass.
  Choose 16-bit WAV with TPDF dither, 24-bit with dither, or mono. Exports tail
  until the song is silent, show progress with a cancel button, report overs,
  and never overwrite a file by default (`song-001.wav`).
- **Sampler: loops and breaks.** Crossfaded loop seams, loop bounds that snap
  to slices or bar divisions, DETECT to find a break's hits and place slice
  markers, PATTERN to write a sliced break into the pattern, key zones
  (several files mapped across the keyboard), and mono legato with glide.

### Instruments

- ML-P8 Unison thickens a note without making it louder. The group shares
  one note's level, so existing songs that used Unison will play quieter.
- ML-P8 Volume, Pan and Spread glide under modulation instead of zippering.
- Ladder and Acid filters put their corner in the same place at every sample
  rate. Resonance tapers evenly to a bounded peak, and 24 dB slopes share
  ML-P8's compensated cascade.
- The sustain pedal, pitch bend, mod wheel and aftertouch work on every
  source. Mod wheel and aftertouch are modulation sources on every channel.
- A stolen or finished sampler voice fades instead of cutting, and voice
  stealing takes releasing voices first.
- The stretch pool follows the sampler's Voices setting instead of holding
  sixteen readers.
- Turning SYNC off keeps the loop's sound, and fit-to-tempo says what it is
  doing.
- The v1 mono and poly synths are gone from the source menus (existing songs
  still open).
- A NaN inside an ML-P8 or DS-01 voice no longer sticks there.

### Effects and containers

- **Layers:** a new container that splits its input across parallel branches
  and sums them. Each branch has S, M and a meter; you can wrap devices in a
  layer, remove a branch, and save a layer as a preset (a bank of three
  ships).
- **Bus Comp:** the master bus compressor, with three laws fitted to
  measurements of the real hardware, its own face with a needle, and a
  lookahead knob. It works on the master and as an insert.
- The master output has a NaN scrub and a 0 dBFS safety limiter.
- The Limiter looks ahead at true peaks, the Gate has hysteresis, and the
  Compressor has its own Mix knob.
- The Drive effect's Drive knob changes character, not level.
- The Filter effect's 24 dB mode and the Delay's feedback are gain-bounded,
  and a NaN can no longer circulate in a filter or delay loop.
- Bypass, removal, wet/dry, trims, container Mix, installing an effect and
  loading a preset all ramp instead of clicking.
- An insert at full wet is its device, to the bit, and a container's input
  and output trims are heard.
- A Buffer keeps its history across a tempo change, an undo and a reload.

### Mixing and routing

- Fader, pan, mute, solo and polarity ramp instead of stepping.
- A channel can be soloed from the chip its mute used to be on.
- Channel volume follows the fader's taper, so a MIDI-mapped fader puts unity
  where the mouse does. A new channel starts at unity.
- A channel's fader and pan can be automated from the lane picker.
- A channel's mixer track is set from the sidebar.

### Sequencing and MIDI

- A MIDI take can replace what each loop pass crosses, and looped passes land
  where they were played.
- Recorded MIDI lands where you heard the song, not where the engine happened
  to be rendering.
- A note's timing no longer depends on where audio block boundaries fall.
- A note the sequencer started is always ended, and a loop wrap no longer
  cuts held keys. Effect tails survive a loop wrap.
- A full pattern refuses a new note and says why, instead of drawing a
  silent one.
- MIDI Start plays from the beginning. Pads can be MIDI-learned, and RELEARN
  replaces its mapping instead of adding a second one.
- A tempo-synced LFO follows the song position, so Play, Seek and an export
  all land on the same phase.

### Interface

- **Every knob** has a right-click menu and takes a typed value.
- A device folds to its header, turned on its side.
- A device is added from the arrow between two devices.
- The left sidebar has a shortcut, plus mute, solo, volume and pan.
- The browser filters. A click selects a preset (and auditions an instrument
  preset), and a double-click or a drop loads it. The browser's arrow keys
  audition what they land on.
- Themes can bevel their surfaces. **Platinum** and **Impulse** ship, and
  **Auto** follows the desktop's light/dark switch live.
- The status bar holds a warning until it is read, and shows the audio
  callback's load and dropouts.
- Questions (unsaved changes, cleaning up takes) are the app's own dialog, not
  a zenity window.
- Super can act as Alt, or swap places with it.
- Every edit is one undo step, including drags, wheel sweeps, automation
  points and toolbar actions. Looking at something (changing the view) is no
  longer an edit.

### Files, sessions and reliability

- **Autosave:** unsaved changes are offered back after a crash.
- Saves are durable (written safely to disk), and only one document operation
  runs at a time. A save adds to the song's assets folder instead of
  rebuilding it.
- A saved effect missing a field still loads, and one that can't be read names
  its kind.
- Takes live outside `~/.config`, are checkpointed every second, and a
  failed one is kept. At quit, mooloop offers to clean up takes nothing uses
  (also under File > Clean Up Takes).
- Every run leaves a log, a panic leaves a crash report, and SIGTERM quits
  cleanly.
- With no JACK running, the app opens and says so. A dead audio engine is
  noticed, and Reconnect brings the song back.
- The audio output follows your most recent choice that is present and never
  moves while connected. A second instance names its own JACK ports. A fresh
  install leaves the JACK buffer size alone.
- The audio thread never frees, reference-counts or locks what it reads, and
  an edit the engine has no room for waits instead of being dropped.
- The deb and rpm packages depend on zenity.

### Performance

Measured on Adam's songs. `housey-dropout-factory` was the worst case.

- **Interface:**
  - Stopped: from 97% of a core to about 2%.
  - Playing: 24–30 fps instead of 7–10.
  - Why: hidden controls are no longer drawn, a running LFO no longer rewrites
    every rack row, and meters no longer re-lay out text.
- **Audio, overall:** songs render slightly faster than on 2026-09-14,
  recovering the 9–20% that two correctness fixes cost on 2026-09-23.
- **Audio, per device:**
  - Drive: 32 → 4 µs a block.
  - Phaser: 46 → 9 µs a block.
  - Sampler voices: 342 → 78 µs for eight.
  - The stretching sampler's worst block: 2.5 → 0.8 ms.
  - ML-P8's filter and oscillators are cheaper.
  - A resting Compressor skips its log maths.
  - A settled insert does no dry copy.
  - Effect displays analyze only while they are on screen.

## 0.1.4 — 2026-09-18

### Highlights

- **A real mixer.** The mixer is made of tracks you create and name. A send is
  a route to a track (a track that receives sends is a return), and every
  track has a channel strip: preamp voicing, EQ, compressor, polarity, solo in
  place, and analog summing.
- **Preamp device.** The input stage as a device you can put on a track,
  with a display that shows where it is distorting.
- **macOS.** A Core Audio driver beside JACK, Core MIDI input, and native
  Save and Open dialogs.
- **MIDI.** A keyboard plays the selected channel. Each channel picks its
  input, and MIDI Learn works by pressing a control and moving a knob. A
  binding notices when something else takes its parameter.
- **Themes.** A theme is a colour scheme, a variant and a typeface. Type,
  stroke and metrics are theme tokens, and the Appearance page previews in
  the chosen scheme.

### Instruments and effects

- The v1 poly synth can be played as a monosynth.
- The EQ gives every band and pass filter its own parameter ids. Its plot
  draws the real filter response, and the bank spreads.
- The Buffer gains Rate, Freeze, Position, Length, Loop and Jump on the song
  grid, a freeze that lands on the bar line, and a 2U face.
- A note played with the transport stopped no longer lasts one block, and a
  sampler note-on never frees a sample buffer on the audio thread.

### Mixing and routing

- Channels and tracks have names and colours. A channel wears its track's
  colour, and a pattern's colour reaches the playlist.
- A channel can be dragged to another row, and a track moves by dragging its
  name plate.
- Sends live in the sidebar, with one bar on the strip.
- Muting the master mutes it (it used to silence only its meters), and a
  muted track's sends drain instead of freezing.

### Interface

- The keyboard reaches the whole application, and Escape leaves a text field.
- One dB vocabulary across the app: no readout rounds for itself.

### Files and reliability

- Channels and tracks have durable identities, so a project edit no longer
  stops the song and installs keep the parts that didn't change.
- Many load and save fixes: range checks on load, renamed songs open with
  their samples, embedded samples are no longer renamed on every save,
  automation lanes in a saved song accept new points, and presets can't
  silently overwrite each other.
- A saved audio output that no longer exists no longer takes the sound with
  it.
- A pattern switch under a running transport no longer strands sounding
  notes, and the playhead no longer stops at bar 64.
- Undo can no longer reach back into a song that is no longer open.

## 0.1.3 — 2026-09-08

### Highlights

- **Containers.** Group a run of devices into a box with its own blend, save
  it as one preset, and drag devices in and out.
- **Panes and views.** Five views across three slots, each with its own
  toolbar. A pane can be split, filled to the window, or have its tab moved
  to another pane, and the layout is remembered.
- **Latency compensation.** Devices declare their latency and the whole tree
  is compensated. The export is checked to be identical to the live render.
- **The first performance pass.** Silent channels, idle effect slots, unused
  buses, disabled EQ bands and sleeping LFOs cost nothing, control nobody
  authored costs nothing, and a song is parsed once on open instead of twice.

### Also

- The ML-P8 factory bank ships. DS-01 publishes six control outlets, and a
  generator's outlets show up as modulation sources. The v1 drum synth gets a
  parameter table.
- A channel can hear another channel's oscillator (Aux In).
- A song can repeat a section, and the playhead can be grabbed and moved.
- Patterns can step a beat at a time, with an accent that doesn't have to be
  on four. The effect's LFO can follow the transport.
- The sampler's Bars knob detents to real loop lengths.
- The browser browses presets, and loading one adds a device. Devices can be
  copied.
- Tooltips are value-only; explanations go to the status bar.
- The clip light latches until cleared, instead of blinking for two seconds.
- The time controls say what the engine is actually doing, and the
  sixteen-channel ceiling is gone.
- The undo history has a ceiling, and the audio callback reports when it was
  late or never ran.

## 0.1.2 — 2026-09-05

### Highlights

- **Three new instruments.** ML-M1 is a mono synth with a four-pole ladder
  and three filter characters (Model switch), accent, and a factory bank.
  ML-P8 is a poly synth with an oscillator network drawn as a network, two
  envelopes, a multimode filter, LFO, internal modulation routes, unison
  voice allocation and paged faces. DS-01 is a drum voice with four
  envelopes, a body resonator, burst, shaper, internal mod matrix and a kit.
- **Modulation rack.** An eight-slot module grid per channel: LFO, envelope,
  step, random and math modules. Any device knob can be a destination, and
  the knob shows its assignment and live movement.
- **Sampler, grown up.** WSOLA time-stretch with a grain mode, pitch and speed
  independent of each other, fit-to-tempo, slices played per note (backwards
  if asked), committing a stretch to the buffer, band-limited playback, a
  filter envelope, and zero-crossing snapping for markers.
- **Gain staging.** A shared gain module. Sources are calibrated to a
  −12 dBFS operating level, faders have a console taper with dB readouts,
  wet paths are level-matched, and peak meters use IEC ballistics.
- **Presets.** Effect presets with a home on disk, and a factory bank for
  every effect kind. Sources wear the preset they came from.

### Also

- The reverb is an FDN hall instead of a convolution player. The plate glides
  size changes and has smoothed predelay.
- Bitcrush has four crusher algorithms, and the dynamics display shows the
  input and a draggable threshold.
- The sample browser docks in the sidebar with preview, autoplay, volume and
  an info pane, and more audio formats are supported.
- The status bar is docked at the bottom, the piano roll dock resizes, and
  panes animate. Preferences > Appearance is rebuilt on three colour seeds.
- The piano roll gains pointer tools, marquee selection, a snap toggle, a
  selection that resizes and scales as one object, and remappable drag
  modifiers. A drag is one undo step.
- A song that can't be saved is kept, and the app logs what it does.
  Documents are repaired on load and save instead of refused.

## 0.1.1 — 2026-08-27

- **The Buffer device:** a retained audio buffer insert with a held, looping
  reverse gesture and an automatable read head.
- **Modulation begins:** channel LFOs wired to effect parameters.
- **Automation:** clip automation lanes, and velocity and automation lanes in
  the piano roll. Sampler, mono and poly parameters are addressable.
- **MIDI input**, with a Buffer control mapping.
- Filter band-pass slope and drive, tempo-synced delay times, and a 2U EQ with
  a selection strip. EQ and Filter share one draggable graph-point handle.
- Effect parameters are smoothed so they no longer zipper, and a smooth
  device-curve preference was added.
- Linux releases build against glibc 2.31.

## 0.1.0 — 2026-08-23

First release.
