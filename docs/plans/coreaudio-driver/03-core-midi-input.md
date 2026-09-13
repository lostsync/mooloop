# 03 Core MIDI input, and a keyboard that plays

Landed 2026-09-13.

JACK gives the engine one MIDI input port a patchbay connects. Core Audio has
no patchbay, so the Core Audio adapter listens to every Core MIDI source
through `midir`, passes messages to the callback over a bounded ring, and
delivers them at the start of the next callback. The driver's `service`, which
already runs from every event poll, rescans the sources once a second, so a
keyboard plugged in later is picked up and an unplugged one is let go.

Delivering MIDI was not enough on its own: decoded messages reached only a
buffer mapping that nothing installs. Adam's ask was to play notes from a
keyboard on the selected channel, so this step also:

- **Routes keys to the selected channel.** `RenderState::apply_midi` sends
  every note a buffer mapping does not claim to the keyboard channel, through
  the queue the slice editor's audition already used, which works with the
  transport stopped. The keyboard's note ids sit in their own block below the
  audition ids.
- **Releases a key where it went down.** A 128-entry table records each held
  key's channel, so moving the selection with a key held does not strand the
  note on the old channel.
- **Follows the selection with an atomic.** `EngineHandle::set_keyboard_channel`
  stores into a cell the render state reads; the UI pump sets it every tick
  from `Session::selected`, which covers every path that moves the selection.
  The cell is re-attached on project install alongside the buffer map.
- **Connects JACK's hardware sources.** The JACK adapter connects every
  physical MIDI output to `mooloop:midi_in` at startup and on port
  registration, so a keyboard plays without a patchbay.

The audition queue grew from 16 to 64 per block, since a dropped entry can now
be a note-off. JACK notes keep their frame offsets; the UI's auditions stay at
offset zero.

Out of scope, and recorded in `CURRENT.md`: CC, pitch bend, sustain and
program change on a channel; recording; a device list or input choice on the
MIDI preferences page; Core MIDI timestamps.
