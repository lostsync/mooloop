# 02 — routing, transport and capture in the engine

**Landed 2026-09-15.**

## The split this step makes

`Renderer::apply_midi` now divides its input two ways, and the division is the
load-bearing decision of the whole feature:

- **Notes are realtime.** A note a buffer mapping claims drives the buffer;
  otherwise every channel whose input claims the message sounds it, at the
  message's own frame offset. If no channel claims it, the selected channel
  plays it — which is exactly what mooloop did before, now as a fallback
  rather than as the only rule.
- **Control is not.** Control changes, pitch bends and transport messages are
  *forwarded* to the control thread as `EngineEvent::ControlInput`, which maps
  them against the project's bindings and issues the same edits the interface
  would.

The second half is the part worth defending. A CC that moved a parameter by a
private realtime path would leave the project holding the old value, the
interface drawing the old value, and undo unable to see the move — and it
would be a *second* implementation of every parameter edit, which is the fault
`AGENTS.md` opens on. The cost is one pump of latency on a knob turn. That is
the right trade for a mapping layer, and a performance subset that needs
tighter timing can be given a realtime fast path later against the same types.

## What that needed

- **`MidiRouting`**, an `ArcSwap`ped table of resolved `MidiInputRoute` by
  channel. Same transport as the buffer map: built and dropped off the audio
  thread, only ever loaded on it. A channel past the end of the table has no
  explicit route and follows the selection, so the table may be short.
- **`HeldKeys`**, a bitset of channels per note, replacing the single byte.
  A note can now go down on several channels at once — that is what
  multitimbral *is* — and the release still goes exactly where the press went.
  The byte could only remember one, so the second channel would have been left
  sounding.
- **A claiming channel is not also played for being selected.** Adding the
  selection to the claimants would double the note on a channel that was both,
  which reads as a stuck 6 dB rather than as a bug.
- **`outgoing`**, a bounded array the executor drains after the block renders,
  leaving two ring slots for position and metering. A desk sending a fader
  stream must not crowd out the block's own truth; what does not fit waits for
  the next block rather than being dropped.
- **Recording.** Armed *and* running, a note-on remembers where it landed on
  the playhead and at what absolute frame; the note-off reports the whole note.
  Length is measured in frames rather than from the tick, so a note held
  across the loop point reports how long it was held instead of a negative
  number. Disarming abandons a note still down rather than reporting a
  half-measured one.

## The drivers

Both now tag each message with a port.

**JACK** has one: every hardware source is auto-connected to `mooloop:midi_in`
and a message arriving there carries no record of its sender, so the picker
gets one honest entry — "All Hardware Inputs" — and two keyboards are told
apart by MIDI channel. A port per source is a driver change and is not in this
plan.

**Core MIDI** connects per source and really can distinguish them, so each
connection gets a monotonic port id. Monotonic and not positional: the
connection list is pruned when a device is unplugged, and a position would then
be reused, so a project naming a port by index would end up pointing at
somebody else's keyboard. Its `Ignore::All` became `Ignore::Sysex |
Ignore::ActiveSense`, because `All` also takes timing and this driver now wants
Start, Continue, Stop and Song Position from that same system range; the clock
itself is dropped in `MidiBytes::new`, where the rule is stated once.

**None of the Core MIDI half has been compiled.** It is behind
`#[cfg(target_os = "macos")]` and this was written on Linux. Build it on macOS
before believing it.
