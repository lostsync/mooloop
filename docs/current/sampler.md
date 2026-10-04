# Sampler

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## The sampler editor

- A sampler editor with waveform, WAV/AIFF/MP3/FLAC/Ogg Vorbis loading and
  mixed-format sibling navigation, trim, reverse, root note, coarse/fine tune,
  loop region and mode, ADSR, low-pass filter with envelope depth and
  resonance, drive, bit reduction, and rate reduction. The filter runs its own
  ADSR, reached through a CURVE/ENV switch on the Tone page's filter panel; a
  patch that never sets one, which includes every project saved before it
  existed, follows the amplitude envelope. An Output trim in the page bar sets
  the patch's level ahead of the channel's inserts; a sampler created today
  starts at -9 dB so a normalized file peaks where the synths' default patches
  do, while projects saved before the trim existed load at unity. Voice
  controls cover one-shot/gated playback, 1-16 voices, restart/layer
  retriggering, and 16 cross-channel choke groups.
  At one voice in Pitched mode the sampler also glides and plays legato,
  with the ML-M1's controls: Glide (0-2 s), GLIDE MODE (Always also
  slides into a release tail, Legato only between held notes) and ENV TRIG
  (Retrig restarts the sample and envelopes on every note; Legato only moves
  the pitch of the note already sounding, so a line keeps its place in the
  sample). Releasing the newest key while an older one is held falls back to
  the older pitch with no new attack. The three are greyed out with more than
  one voice or in Slice mode. A patch with no glide and Retrig plays exactly as
  a plain one-voice sampler, so older songs don't change.
  Key zones, on the ZONES page: the sampler's own sample plays a key
  range (LOW and HIGH), and + ZONE adds another audio file as a zone with its
  own key range and ROOT. A note plays the sample if its range holds the
  note, otherwise the first zone whose range holds it, otherwise nothing. A
  zone is pitched from its own root. A zone is removed with its x, and every
  edit is one undo step. The first zone added to a full-keyboard sampler
  takes the upper half of the keys. A later zone takes the keys above every
  range, or splits the top range when there are none. Each zone is its own
  region of its file: start, end, reverse, loop points, loop mode
  and fade, root, tune and a LEVEL trim. A ZONE strip in the device's page
  bar (on SAMPLE and VOICE, once a sampler has zones) picks the zone those
  pages edit, and so does clicking a zone's name on ZONES; the sampler's own
  sample is zone 1. The waveform, markers, ROOT, reverse, loop, tune and
  LEVEL then show and set that zone alone, and only its voices draw a
  playhead. FOLLOW, off by default, selects the zone of each key as it
  starts sounding. Envelopes, filter, drive and crush, voice, glide, stretch
  and play mode are shared by every zone. A new zone plays its whole file. A
  lane or modulation route on start, end, loop or tune moves every zone by
  the same amount from its own setting. Slices and the stretch commit belong
  to zone 1. Slices and zones never both apply: a sampler with
  zones is greyed out of Slice mode (a MIDI mapping or lane on Play mode
  cannot put it there either), its markers and DETECT are unavailable, and
  in Slice mode + ZONE and DUPE are greyed out. A song saved since 0.1.5
  with both plays its slices, its zones unheard, until it is switched to
  Pitch. Many zones can share one file: DUPE copies the selected
  zone, its file and region, onto the next free key (rooted to play what the
  original's lowest key did; with no key free it takes the upper half of the
  original's range), and selects it. On an unzoned sampler with slice
  markers, ZONES FROM SLICES makes one zone per slice, one key each from the
  FROM key (the slice base note to start), each rooted on its own key so
  every hit plays at its recorded pitch, with zone 1 the first slice; the
  sampler leaves Slice mode and live stretch is switched off. Each is one
  undo step, and no file is edited. Zones on one file share its decoded
  audio, in the session and on load. A zone saved by 0.1.5 keeps the shared
  region it played, as its own. Zones embed with the song like its sample,
  and a file several zones share is embedded once. A copied sampler
  channel carries its zones' audio, so it pastes with every zone playing
  into a song opened or created since the copy. A zone whose file is
  missing says so on the page and plays nothing. Zones store a velocity range
  that nothing plays yet (velocity layers). The full mapping workspace
  (MOO-40) and SFZ import (MOO-41) are not built.

## Loops, slices and voices

- **A forward loop's seam can be crossfaded.** Loop fade, beside the loop
  mode on the sampler's header, is 0 to 100 ms of the sample's own time. The
  loop's last stretch blends, equal-power, into the material just before its
  start, so its last frame is its first frame's neighbour and the seam has
  nothing to click on. The loop keeps its length and its start (a break's
  downbeat) plays as it always did. A loop that starts at the region's first
  frame has no material before it, so its end fades out to silence and its
  first millisecond fades back in. The waveform shades the span the fade
  covers. Reverse crosses the same blend, ping-pong has no seam, a fade is
  never more than half the loop, and 0 ms (the default, and every song saved
  before the fade existed) is the hard seam, bit for bit. The fade is a
  descriptor, so it can be automated and modulated.
- **Fit to tempo says what it's doing, and SYNC off keeps the sound.** With
  SYNC on, a line under Bars reads the loop's own length and tempo and what
  it lasts fitted, for example "1.00 s at 120.0 BPM → 2.00 s". It turns the
  warning colour when the bar count gives the loop a tempo outside 60-200
  BPM, or needs more stretch than the sampler has, and the status bar then
  says why and suggests a bar count. The Bars field also takes the loop's own
  tempo, "96 bpm", and turns it into bars. Turning SYNC off writes the ratio
  it was running into the Speed knob, in the same undo step, so the loop
  keeps sounding the same until the knob is touched. That ratio is the root
  key's at the current tune; a transposed note plays shorter or longer from
  then on, which is what a fixed ratio means. A fitted loop renders the same
  offline as live.
- **A loop's bounds can snap to a grid.** Loop grid, beside the L/R fields,
  is Free, Slices, or 1 bar down to 1/32. A division is of the sample's own
  bar count (the Bars fit-to-tempo uses), counted from the playback region's
  start. Slices snaps to the slice markers and the region's ends. It applies
  wherever the bounds come from, the markers, a lane or a modulator, so a
  lane sweeping Loop start steps through the grid in rhythm instead of
  sliding through every frame. The loop band is drawn at the snapped bounds.
  A grid can't collapse or invert the loop: a loop shorter than one step
  becomes one step. A grid with fewer than two points inside the region
  leaves the loop free. Loop grid is automatable and not modulatable, and
  Free (the default, and every older song) is the loop as it always was.
- **Slices can be detected.** DETECT in Slice mode finds the hits in the
  playback region and previews a ghosted marker on each. Nothing changes
  until the preview is accepted. While it's up, the slice row holds
  Sensitivity (how quiet a hit still counts) and Spacing (the closest two
  markers may land, 10-250 ms). Each of them re-detects. REPLACE keeps the
  markers placed or moved by hand and replaces the rest, MERGE keeps every
  marker and adds the detected ones that aren't beside one, and CANCEL
  changes nothing. Either accept is one undo step. The detector measures both
  channels and a high-passed copy of each, so a hit panned to one side counts
  and a steady tone doesn't. Each marker lands just before its attack.
  Detected markers are ordinary markers. Whether a marker was placed by hand
  is saved, and every marker in an older song counts as hand-placed.
- **A sliced break becomes a pattern.** PATTERN, in Slice mode, writes one
  note per slice into the channel's current pattern, at the tick the slice
  falls on in the break. The break is the playback region, `Bars` long. Each
  note is the base note plus the slice's position, the same mapping the
  keyboard plays, and lasts until the next slice. Placement keeps the break's
  own timing unless 1/16 is lit. REPLACE clears the channel's notes in that
  pattern first, and ADD writes beside them. A pattern shorter than the break
  grows to hold it. Either is one undo step. Slices past MIDI 127, or past
  the longest a pattern can be, are left out and counted in the status bar.
  The notes are ordinary pattern data from then on.
- Sampler voice allocation is fixed-capacity and deterministic: restart reuses
  the oldest matching pitch, layer mode overlaps notes, and overflow steals
  a releasing voice before a held one, the oldest of either. The sampler, the
  Poly Synth and the ML-P8 all steal in that order.
- **Nothing a voice does ends mid-waveform.** A voice the sampler steals --
  which on the default patch, one voice in Restart, is every new note --
  moves to one of the sixteen slots above the Voices count and fades there
  over the choke's 5 ms while the new note starts fresh in its place. A
  non-looping region or slice fades over its last 2 ms (at most a quarter of
  a short slice) instead of stopping on whatever sample it held. Lowering
  Voices on the sampler or the Poly Synth, or switching the Poly Synth to
  Mono, fades the voices it retires.

## Recording into the sampler

Clip recording, the way Ableton's or Bitwig's clip mode records and Maschine's
sampler does:

- **Every channel has an AUDIO row** in the channel sidebar, beside MIDI IN
  and independent of it: Off, the hardware input (below), the master, any
  track, any channel -- itself included. It names its source by identity, so
  it follows moves, and says so when the source has been deleted. Any number
  of channels may hold one, and it stays set.
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
- **Takes a crash left unfinished open again.** At startup every
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
- **File > Clean Up Takes** (`recording.clean-up`) opens the same
  dialog at any time, with two lists. *Not used by this song*, ticked: this
  session's takes in the shared folder that nothing reaches, and takes in the
  song's own `recordings/` that neither the open song, its undo history nor
  the song as saved on disk plays. *Left from earlier sessions*, unticked and
  saying why: shared-folder takes older than this run, which a crash (or a
  quit while only the undo history used them) leaves behind and which may be
  the only copy of a take from a song that was never saved. Stretch renders
  in the shared `renders/` folder are listed on exactly the same
  terms as takes in the shared recordings folder: one the open song, its undo
  history, the clipboard or an autosave uses -- as its sample or as a
  commit's original, which REVERT goes back to -- is never offered. Nothing
  moves until **Move to Trash**, and then only to the trash. The quit offer
  lists takes only.
- **The hardware input is an AUDIO source** under both drivers. Under JACK
  it is "Audio In": `mooloop:in_l`/`in_r` wired to the first physical capture
  pair. Under Core Audio it is the system's default input device, listed by
  that device's own name -- Core Audio opens a device rather than joining a
  graph, so there is a device to name. A take from it starts the round-trip
  latency after its bar, so it lines up with what was played. With it picked,
  the AUDIO row shows a peak meter and **MON**, which plays the input through
  the channel -- off by default, not saved, and never switched on by anything
  else, because a microphone through speakers feeds back.
- **On a Mac the input row appears only if there is an input.** No input
  device, a device that will not run at the engine's sample rate, or a
  microphone macOS has not granted mooloop, and the AUDIO row simply lists no
  input; the reason is logged once at startup. macOS asks for the microphone
  the first time mooloop opens it, and refusing it is not an error the
  interface reports anywhere else.

## Slicing and stretch

A stretching sampler holds stretch state for twice its Voices count rather
than for all sixteen. Changing Voices resizes it within a pump tick, whether
the change came from the stepper, a MIDI-learned control, a preset or an
undo, and the voices already sounding keep their state through the resize.
The spare half is where a stolen voice fades out. A channel with an
automation lane on Voices holds all sixteen, because a lane moves Voices on
the audio thread, where nothing can be allocated.

Known gaps:

- **Live stretch is bypassed, not refused, in Slice mode, in reverse, and in
  Pong.** The DSP declines to run WSOLA backwards and the commit path is the
  answer, and the face still shows the ON toggle lit while nothing stretches.
  The toggle says which of the three it is, and to commit, in the status
  bar.
- **A REVERT onto an original that changed on disk places markers by
  proportion.** The traces a revert maps markers through are re-made from the
  original, so an original replaced since the commit cannot reproduce them;
  the revert still happens, maps the markers by the share of the length they
  sat at, and says so in the status bar.

**COMMIT renders the sample and loads the render in its place** (Adam's
ruling: a committed sample is treated like a rendered one). It renders the
whole sample, not just the playback region, so every marker has somewhere to
go: slices, Start/End and the loop points all move to where the stretch put
them, and none is dropped, so a slice keeps its key and a slice pattern keeps
playing the same hits. The render is written when it is made to a shared
`renders/` folder in the data directory, beside `recordings/`
(`~/.local/share/mooloop/renders/`), and becomes the channel's sample: the
song owns it the way it owns a take, a save copies it into the song's
`samples/`, and a reload plays the stored file rather than re-rendering
anything. If it cannot be written the commit still happens and the status bar
says why; the song then keeps the original and re-makes the render from it on
load. The live stretch is switched off and a free ratio set back to 1, since
the stretch is in the audio. Edits after a commit are ordinary edits of the
sample on screen.

**The badge after a commit reads stale when committing again would stretch
the committed audio**: the tempo moved under a fitted loop, a marker a
Slices-snapped loop sits on moved, or the free ratio was turned. A change of
stretch mode or grain alone is not stale, because the audio is already baked.
**REBAKE** then commits again: it stretches the committed audio by what is
missing, so commits stack and nothing made since is thrown away; there is no
re-render from the original. **REVERT** goes back to the original sample (from
before the first commit) in one click, with the markers on screen mapped back
through every commit, so slices and trims made since come too; the live
stretch goes back on at the ratio the commits baked. A song saved by 0.1.5
with a commit loads and plays the render it played then.
