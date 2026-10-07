# Sequencing

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

## Channels, patterns and the step grid

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
- The complete 256-channel addressable bank. A new song starts with the same
  four-channel DS-01 drum machine kit every time (Kick, Snare, Closed Hat and
  Open Hat, from the factory patches Machine Kick, Machine Snare, Machine Hat
  and Machine Open Hat, whose names each channel's generator wears), grouped
  onto one `Drums` track. Channels can use any of six sources — the sampler,
  the Gitdum DS-SX (the v1 drum synth), the Dominic DS-01, the Munotone
  ML-M1, the Polyneight ML-P8, or Aux In, which plays another channel's
  published audio outlet — and every rack row exposes solo and mute, output
  volume, and constant-power stereo pan. A source wears its full name where
  there is room and its model number where it is tight (Adam's ruling): the
  rack's `+`, the source picker's menu, the device header and the preset
  browser's groups read "Polyneight ML-P8"; the 96px picker chip, a new
  channel's name ("ML-P8 2") and the automation and MIDI lists read "ML-P8".
  These are display names only: a song's stored channel names, such as an old
  "Drum Synth 1", are kept as saved. The rack's `+` offers those six when
  adding a channel, which is the same list the source picker offers when
  changing one; its rows are ordinary menu rows, reading down a left edge like
  the rest of the interface's menus. The v1 mono and poly synths are not
  offered in either (the ML-M1 and ML-P8 cover them): a song that uses one
  loads and plays unchanged, and its channel's picker still lists it while
  selected.
- Channels can be reordered by dragging a rack row's name plate. The rows
  between the grab and the landing slide aside, and the gap that opens is the
  drop indicator. Every address in the song that named a channel follows it —
  automation lanes, modulation routes, an Aux In's subscription — and so does
  the session's own state: the selected device, the open automation lane, and
  the preset labels a channel and its rack rows are wearing. The move is one
  undoable edit, and nothing stops for it: every channel, the moved one and
  the ones it passed, keeps its sounding notes, tails and modulators.

- Patterns are created explicitly from a one-pattern project, with up to 256
  addressable pattern IDs and independent logical lengths from 1 to 256 steps.
  Hidden steps survive shortening and re-extending a pattern.
- **Channels, tracks and patterns can each be named, and the names are
  saved.** A channel is renamed on the `DEVICES` toolbar, in the channel
  sidebar's NAME row, or in the step rack: a double-click on a channel's
  plate turns it into a name field, and Tab or Shift+Tab moves the field to
  the next or previous channel, so a run of channels can be named from the
  keyboard. Enter, Escape or a click elsewhere closes it, and Tab past the
  last channel does too. Each channel's rename is its own undo step. A track
  is renamed on its own device face, a pattern in the transport toolbar. A
  channel or a track refuses a blank name, because its rack plate or its mixer
  column is the only thing identifying it; a pattern accepts one and reads as
  `Pattern N` wherever it is drawn -- the pattern menu and the playlist's
  gutter -- because its number is beside it there. A channel keeps the name it
  was given when its source device is changed: only a channel still wearing
  the outgoing device's default name is renamed after the new one. Cloning a
  pattern gives the copy its name and colour, and cloning or deleting one
  leaves every other pattern's name on that pattern.

## Notes and the piano roll

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
  Ctrl, and Alt. Shift is snap override alone, because a Shift that also
  added to the selection deselected the note a Shift-drag was about to move
  and carried it off on its own. The snap override inverts the toggle rather
  than only defeating it, so it frees a drag when snap is on and quantises one
  when it is off.
- How a Super (Meta/Win) press is read is a preference on that same page:
  separate keys, which is the default and what every binding assumes; Super
  acting as Alt, where either key presses an Alt chord; or Alt and Super
  swapped. It changes no binding -- only which physical key reaches one -- so
  every chord the page lists goes on saying what it said. It is for a desktop
  whose window manager takes Alt before mooloop sees it, and for a keyboard
  with the two keys transposed. Pointer gestures are unaffected: a gesture
  role can already be assigned Meta outright.
- Two lane areas sit under the roll and toggle independently: a velocity
  lane drawn as stems with drag heads, and the automation lanes. Every open
  automation lane of the clip is shown, stacked, each with a header whose
  menu clears or removes it; the area scrolls once it reaches its height,
  which its top edge sets. A lane's bottom edge resizes it, and Shift-drag
  resizes every lane (view state, not saved). **Add lane** opens two
  columns: devices (the selected channel's generator, each effect on that
  channel, the channel's fader and pan as "Channel strip", and each effect
  on every bus), and the hovered device's parameters. **Open lanes** heads
  the device column whenever the clip has a lane, listing just those;
  devices and parameters with a lane are marked, and picking an open one
  focuses it. Points are
  drawn by clicking, dragged to move (Ctrl for a tenth of the travel),
  right-clicked to remove, and interpolate linearly. Values snap to whole
  semitones or cents on a semitone or cent amount and to the positions of a
  stepped parameter; Shift frees both value and time. Value lines mark
  quarters with the middle stronger, octaves on a semitone amount, and each
  position of a stepped parameter with up to seventeen.
- Sixteenth-note rack cells summarize their four 64th-note substeps without
  discarding rests between hits. Each substep is drawn solid where a note is
  struck and dim where one is merely held, so a ratcheted step is
  distinguishable from a single sustained note; coverage alone renders both as
  a full cell.

## Events and voices

- Probability, microtiming controls, ties, and parameter locks are not yet
  implemented. Note starts and lengths otherwise retain PPQ precision.
- **A note the sequencer started is always ended.** Each channel keeps a
  table of the voices its pattern started, and every edit that takes a
  note-off out of the playhead's reach releases the voice: deleting,
  shortening, moving or re-pitching a sounding note, a pattern-length change,
  a placement removed, a playback-mode or pattern switch, and a mute or solo.
  Lengthening a sounding note or changing its velocity leaves it ringing. A
  note that ends while its channel is muted is still ended, so unmuting does
  not bring back a frozen voice. A Song-mode loop fold releases the pattern's
  voices with a note-off rather than choking every channel, so a chord the
  player is holding rings across the loop point and a pad's release tail
  rings over it. **Panic (All Notes Off)** is a bindable action with no
  default chord: it ends every voice and every held key, pedal included,
  without stopping the song.
- **Cloning, clearing or deleting a pattern under a playing song** reaches
  the engine as one edit rather than a rebuild of the song. The cleared or
  deleted pattern's sounding notes end; in Song mode so do those of every
  pattern the edit renumbers (the ones after a clone or a deletion), and in
  Pattern mode a clone made current plays on from where the original was. A
  knob a cleared or deleted pattern's lane was driving under the playhead
  returns to its own value, as it does when you switch away from that
  pattern. Undo still rebuilds.
- One channel holds at most 1,024 notes in one pattern (the engine's
  preallocated store; `docs/CAPACITY_POLICY.md`). Every way of adding a note
  -- drawing, painting, a step, a step slice, a roll slice, duplicate, paste
  and recording -- refuses at the cap and the status bar says why; a paste or
  duplicate that would cross it is refused whole rather than cut short.
- NoteOn, NoteOff, and choke events are sample-accurate and deterministically
  ordered. One-shot loops exit into their remaining sample tail; gated loops
  release through the amplitude envelope.

## Transport and arrangement

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

- Pattern mode loops the selected pattern. Song mode layers playlist placements
  on the shared absolute clock and loops at the bar after the furthest clip end.
- A song loop repeats a section of the arrangement instead. It is stored with
  the song in absolute PPQ ticks, applies in Song mode only, and never applies
  to an offline render, which walks the arrangement once from the top. A loop
  reaching past the song's own end plays the part of it that exists, so
  shortening a song under a loop stops the loop rather than being refused.
  At a song loop's fold, and when the current pattern is switched **in
  Pattern mode** under a running transport, the voices the sequencer started
  are sent a note-off, because the note-off each was waiting for is no longer
  on the way. Their release tails ring, and a key the player holds or an
  audition carries on (*Events and voices*, above). A seek chokes every voice
  on every channel. Pattern mode's own wrap, and Song mode's at the end of the
  song with no loop, are not folds: nothing is released, and a note that
  outlasts the pattern ends at its own note-off on the next pass.
- **Selecting a pattern is a view change everywhere else, and costs nothing.**
  In Song mode the selection is not what is playing -- it is what the editor
  draws and where a recorded note goes -- so switching it while the song runs
  releases nothing. Nor does switching with the transport stopped, where an
  audition or a held key belongs to the player rather than to the pattern being
  left, nor re-selecting the pattern already current, nor a selection past the
  end of the bank.
- **Moving around the app does not interrupt what is playing**, and that is
  a rule. Selecting a pattern, a channel, a bus, a device or a track is a view
  change: it changes what is drawn and what the next edit will address, and
  none of it reaches the audio thread. The one gesture that must tell the
  engine anything is the pattern selection, because the active pattern is
  also where a recorded note goes -- and the engine charges for what changed
  rather than for the fact that a command arrived.
  `scripts/dupe-audit navigation-sends` reports a selection handler that
  breaks the rule.
- **A seek does not ring the old position over the new one.** Delay,
  modulation, reverb and plate tails are cleared when the transport is seeked
  or stopped, because what they hold is audio from a part of the song that is
  no longer playing. An unsynced LFO free-runs through a seek. A
  **tempo-synced LFO follows the song position**: while the transport runs
  its phase is re-derived from the position in beats every control tick, so
  Play, Stop-and-play and Seek land it where the position implies, and an
  export -- which builds fresh modulators at the top -- hears the phase
  playback did. Stopped, it free-runs; one set to retrigger on notes follows
  the notes instead.
- **Tails survive a loop fold and a pattern switch.** A delay repeat or a
  reverb tail from the end of a song loop wraps into its start, the way a
  groove box plays a loop. A Pattern-mode pattern switch under a running
  transport does not clear them either.
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
  placements, so the buried one can still be removed. The timeline is
  horizontally zoomable. Global swing delays alternate sixteenth notes from
  50% (straight) through 75% (strong shuffle), preserving note duration in
  realtime and offline rendering. There is no clip dragging, time-signature
  model, groove template, per-pattern swing override, or per-channel timing
  offset.

- There is no metronome. The toolbar deliberately does not offer a click-track
  toggle, since nothing in the DSP graph produces one yet.
