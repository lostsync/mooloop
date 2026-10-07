# MIDI

Part of [CURRENT.md](../CURRENT.md): what the application does today in this
area, and where each behaviour stops.

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
  rather than asking the device. One pedal serves every input, and
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
  whatever its instrument: an inlet tag on the patch canvas reads *mod wheel*
  or *aftertouch*, and arms and routes to any knob the way a box does. Aftertouch is channel pressure or,
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
  recording.** A channel picks its input and an Omni-or-1–16 channel filter
  from the sidebar's IN and CH rows, and the engine routes notes by them; a
  stored port that is not plugged in says so under the picker rather than
  leaving the channel silently unplayable. A record-arm button sits beside
  play and stop — arming, not recording, so arming while stopped works — and
  an armed transport captures played notes into the pattern selected when the
  key went down, even if the selection moves before it comes up. In pattern
  mode a note lands where it was played in the loop; in song mode it lands at
  its offset in the placement of the selected pattern under the playhead, and
  a note played where no placement of that pattern is playing is heard but
  not recorded.
  **OVR/REPL** beside the arm picks what a take does to the notes already
  there. **Overdub**, the default, adds every pass.
  **Replace** removes the recording channel's notes in the pattern being
  recorded into as the playhead crosses their start, so each pass replaces
  the one before and only the last is left; a note is never removed by the
  pass that played it. Only continuous playing crosses anything: a locate,
  a stop, a pattern switch or a jump of more than a bar does not, and the
  song loop's own jump back does. The recording channel is fixed when the
  take starts: every channel whose own MIDI input takes the keyboard, or the
  selected channel when none does. A pass where nothing is played still
  clears what it crosses. The
  removal happens in the 8 ms pump, so an old note still sounds if the
  playhead reaches it before the pump does. A take, removals and all, is one
  undo step. The mode is not saved, like the arm.
  A recorded note is stamped earlier by the driver's playback latency, so it
  lands where the player heard the song rather than where the engine was
  rendering it: JACK's reported playback latency on the
  output port, or on Core Audio the output device's latency, safety offset,
  IO buffer and stream latency as the HAL reports them, summed (one output
  buffer if any of those reads fails), read again once
  a second. Core Audio's number is logged each time an output stream opens
  (`output latency on ...` in the diagnostic log). Its input latency, which
  starts an audio take late, adds the input device's latency, safety offset
  and stream latency the same way.
  With no device nothing moves. In pattern mode a note pulled before the
  pattern's top folds into the end of the pass before; in song mode one
  pulled off the front of a placement, where no placement of the pattern
  plays, lands on that placement's start.
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
  it is touched (a control resting within one step of the value takes over at
  once), and a control that has taken over gives the parameter back
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
